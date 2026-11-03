use std::io::{self, Read, Write};
use webrtc_vad::{SampleRate, Vad, VadMode};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brief_noise_during_a_chat_pause_does_not_hold_the_turn_open() {
        let mut gate = Turn::new(true);
        assert_eq!(
            (0..8).filter_map(|_| gate.push(true)).last(),
            Some(Edge::Start)
        );
        for _ in 0..3 {
            for _ in 0..15 {
                assert_eq!(gate.push(false), None);
            }
            for _ in 0..4 {
                assert_eq!(gate.push(true), None);
            }
        }
        assert_eq!(
            (0..80).filter_map(|_| gate.push(false)).last(),
            Some(Edge::End)
        );
    }

    #[test]
    fn short_noise_doesnt_interrupt_and_a_thinking_pause_stays_in_one_turn() {
        for (conversation, quiet) in [(false, 40), (true, 125)] {
            let mut gate = Turn::new(conversation);
            let mut advance = |frames, speech| {
                (0..frames)
                    .filter_map(|_| gate.push(speech))
                    .collect::<Vec<_>>()
            };
            assert!(advance(3, true).is_empty());
            assert!(advance(20, false).is_empty());
            assert_eq!(advance(10, true), [Edge::Start]);
            assert!(advance(quiet - 5, false).is_empty());
            assert!(advance(15, true).is_empty());
            assert!(advance(quiet - 1, false).is_empty());
            assert_eq!(advance(1, false), [Edge::End]);
            assert!(advance(100, false).is_empty());
            assert_eq!(advance(10, true), [Edge::Start]);
        }
    }
}

#[derive(Debug, PartialEq)]
enum Edge {
    Start,
    End,
}

struct Turn {
    active: bool,
    consecutive: usize,
    conversation: bool,
    speech_run: usize,
}

impl Turn {
    fn new(conversation: bool) -> Self {
        // 20 ms frames: allow 2.5 s to think in chat, 800 ms for review comments.
        Self {
            active: false,
            consecutive: 0,
            conversation,
            speech_run: 0,
        }
    }

    fn push(&mut self, speech: bool) -> Option<Edge> {
        // In chat, a short burst during a pause is not enough to resume speech.
        if self.conversation && self.active && speech {
            self.speech_run += 1;
            if self.speech_run < 8 {
                return None;
            }
        } else {
            self.speech_run = 0;
        }
        if speech == self.active {
            self.consecutive = 0;
        } else {
            self.consecutive += 1;
            let threshold = match (self.active, self.conversation) {
                (false, _) => 8,
                (true, true) => 125,
                (true, false) => 40,
            };
            if self.consecutive >= threshold {
                self.consecutive = 0;
                self.active = speech;
                return Some(if speech { Edge::Start } else { Edge::End });
            }
        }
        None
    }
}

// Frames on stdout: little-endian u16 byte count, then that many PCM bytes.
// 2 bookmarks the first voiced frame before debounce; 0 confirms a turn; 1 ends it.
pub fn stream(
    mut input: impl Read,
    mut output: impl Write,
    duplex: bool,
    conversation: bool,
) -> io::Result<()> {
    let format = aec3::nodes::audio::AudioFormat::ten_ms(16000, 1);
    let mut echo = if duplex {
        Some(
            aec3::pipelines::linear::builder(format, format)
                .enable_gain_controller2(false)
                .build()
                .map_err(io::Error::other)?,
        )
    } else {
        None
    };
    let mut vad = Vad::new_with_rate_and_mode(SampleRate::Rate16kHz, VadMode::Aggressive);
    let mut chat_vad = conversation
        .then(silero_vad_crs::SileroVad::new)
        .transpose()
        .map_err(io::Error::other)?;
    let mut pending = Vec::new();
    let mut chat_speech = false;
    let mut turn = Turn::new(conversation);
    let mut history = std::collections::VecDeque::with_capacity(20);
    let mut frame = [0u8; 640];
    loop {
        let mut stereo = [0u8; 1280];
        match input.read_exact(if duplex {
            &mut stereo[..]
        } else {
            &mut frame[..]
        }) {
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            other => other?,
        }
        if let Some(echo) = &mut echo {
            for (pair, mono) in stereo.chunks_exact(640).zip(frame.chunks_exact_mut(320)) {
                let sample = |i| f32::from(i16::from_le_bytes([pair[i], pair[i + 1]])) / 32768.0;
                let render: [f32; 160] = std::array::from_fn(|i| sample(i * 4 + 2));
                let capture: [f32; 160] = std::array::from_fn(|i| sample(i * 4));
                let mut clean = [0.0; 160];
                echo.handle_render_frame(&render)
                    .map_err(io::Error::other)?;
                echo.process_capture_frame(&capture, &mut clean)
                    .map_err(io::Error::other)?;
                for (dst, value) in mono.chunks_exact_mut(2).zip(clean) {
                    dst.copy_from_slice(&((value * 32768.0).round() as i16).to_le_bytes());
                }
            }
        }
        let samples: [i16; 320] =
            std::array::from_fn(|i| i16::from_le_bytes([frame[i * 2], frame[i * 2 + 1]]));
        let speech = if let Some(detector) = &mut chat_vad {
            // Silero scores 32 ms at a time; keep its last decision between windows.
            pending.extend(samples.iter().map(|&s| f32::from(s) / 32768.0));
            let n = detector.source_window_samples();
            if pending.len() >= n {
                chat_speech = detector
                    .forward_chunk(&pending[..n])
                    .map_err(io::Error::other)?
                    >= 0.5;
                pending.drain(..n);
            }
            chat_speech
        } else {
            vad.is_voice_segment(&samples).expect("20 ms frame")
        };
        if speech && !turn.active && turn.consecutive == 0 {
            output.write_all(&2u16.to_le_bytes())?;
            output.flush()?;
        }
        if !turn.active {
            if history.len() == 20 {
                history.pop_front();
            }
            history.push_back(frame);
        }
        match turn.push(speech) {
            Some(Edge::Start) => {
                output.write_all(&0u16.to_le_bytes())?;
                for prior in history.drain(..) {
                    output.write_all(&640u16.to_le_bytes())?;
                    output.write_all(&prior)?;
                }
            }
            Some(Edge::End) => output.write_all(&1u16.to_le_bytes())?,
            None if turn.active => {
                output.write_all(&640u16.to_le_bytes())?;
                output.write_all(&frame)?;
            }
            None => continue,
        }
        output.flush()?;
    }
}
