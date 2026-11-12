// Pipecat Smart Turn v3.2: classify a pause, never run inference on the PCM reader.
use ort::{session::Session, value::Tensor};
use rustfft::{FftPlanner, num_complex::Complex32};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::mpsc::{self, Receiver, SyncSender},
    thread,
};

pub(super) struct SmartTurn {
    frames: VecDeque<[i16; 320]>,
    jobs: SyncSender<(u64, Vec<f32>)>,
    scores: Receiver<(u64, f32)>,
    generation: u64,
    asked: bool,
    voiced_frames: usize,
    confirmed: bool,
}

impl SmartTurn {
    pub(super) fn new(resources: PathBuf) -> Self {
        let (jobs, rx) = mpsc::sync_channel::<(u64, Vec<f32>)>(1);
        let (tx, scores) = mpsc::channel();
        thread::spawn(move || -> Option<()> {
            // A warmed microphone in an idle window never loads the model.
            let mut job = rx.recv().ok()?;
            let session = || -> Result<Session, Box<dyn std::error::Error>> {
                ort::init_from(resources.join("../Frameworks/libonnxruntime.dylib"))?.commit();
                Ok(Session::builder()?
                    .with_intra_threads(1)?
                    .with_inter_threads(1)?
                    .commit_from_file(resources.join("smart-turn.onnx"))?)
            };
            let mut session = match session() {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Smart Turn unavailable; using silence: {e}");
                    return None;
                }
            };
            loop {
                let (generation, samples) = job;
                let score =
                    Tensor::from_array(([1usize, 80, 800], features(&samples))).and_then(|x| {
                        Ok(session.run(ort::inputs![x])?[0]
                            .try_extract_tensor::<f32>()?
                            .1[0])
                    });
                match score {
                    Ok(p) => tx.send((generation, p)).ok()?,
                    Err(e) => {
                        eprintln!("Smart Turn inference failed; using silence: {e}");
                        return None;
                    }
                }
                job = rx.recv().ok()?;
            }
        });
        Self {
            frames: VecDeque::with_capacity(400),
            jobs,
            scores,
            generation: 0,
            asked: false,
            voiced_frames: 0,
            confirmed: false,
        }
    }
    pub(super) fn clear(&mut self) {
        self.frames.clear();
        self.voiced_frames = 0;
        self.confirmed = false;
        self.generation += 1;
        self.asked = false;
    }
    pub(super) fn buffer(&mut self, frame: [i16; 320]) {
        if self.frames.len() == 400 {
            self.frames.pop_front();
        }
        self.frames.push_back(frame);
    }
    pub(super) fn push(&mut self, frame: [i16; 320], speech: bool, quiet: usize) -> bool {
        self.buffer(frame);
        if speech {
            self.voiced_frames += 1;
            self.confirmed = false;
            self.generation += 1;
            self.asked = false;
        } else if quiet >= 20 && self.voiced_frames >= 45 && !self.asked {
            // Let short openers keep the full thinking pause. A snapshot is cheap
            // compared with running the model on every 20 ms microphone frame.
            let audio = self
                .frames
                .iter()
                .flatten()
                .map(|s| f32::from(*s) / 32768.)
                .collect();
            self.asked = true;
            let _ = self.jobs.try_send((self.generation, audio));
        }
        self.confirmed |= self
            .scores
            .try_iter()
            .any(|(g, p)| g == self.generation && p >= 0.95);
        // A confident score alone split a real dictation in the middle of a thought.
        // Leave 1.6 s for the speaker to continue; the ordinary timeout is 2.5 s.
        self.confirmed && !speech && quiet >= 80
    }
}

// Match the upstream Whisper frontend: left pad to 8 seconds, normalize, then
// a centered 400-sample periodic Hann transform and 80 Slaney mel bands.
fn features(audio: &[f32]) -> Vec<f32> {
    const N: usize = 128000;
    let mut x = vec![0.; N];
    let n = N.min(audio.len());
    x[N - n..].copy_from_slice(&audio[audio.len() - n..]);
    let mean = x.iter().map(|&v| v as f64).sum::<f64>() / N as f64;
    let scale =
        (x.iter().map(|&v| (v as f64 - mean).powi(2)).sum::<f64>() / N as f64 + 1e-7).sqrt();
    x.iter_mut()
        .for_each(|v| *v = ((*v as f64 - mean) / scale) as f32);
    let step = 6.4_f64.ln() / 27.;
    let mel_max = 15. + 8_f64.ln() / step;
    let hz: Vec<_> = (0..82)
        .map(|i| {
            let m = mel_max * i as f64 / 81.;
            if m < 15. {
                m * 200. / 3.
            } else {
                1000. * ((m - 15.) * step).exp()
            }
        })
        .collect();
    let bands: Vec<Vec<_>> = (0..80)
        .map(|b| {
            (0..201)
                .filter_map(|k| {
                    let f = k as f64 * 40.;
                    let w = ((f - hz[b]) / (hz[b + 1] - hz[b]))
                        .min((hz[b + 2] - f) / (hz[b + 2] - hz[b + 1]));
                    (w > 0.).then_some((k, (w * 2. / (hz[b + 2] - hz[b])) as f32))
                })
                .collect()
        })
        .collect();
    let window: Vec<_> = (0..400)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / 400.).cos())
        .collect();
    let fft = FftPlanner::<f32>::new().plan_fft_forward(400);
    let mut buf = vec![Complex32::default(); 400];
    let mut scratch = vec![Complex32::default(); fft.get_inplace_scratch_len()];
    let mut out = vec![0.; 80 * 800];
    let mut peak = f32::NEG_INFINITY;
    for t in 0..800 {
        for i in 0..400 {
            let j = (t as isize * 160 + i as isize - 200).unsigned_abs();
            buf[i] = Complex32::new(x[if j < N { j } else { 2 * N - 2 - j }] * window[i], 0.);
        }
        fft.process_with_scratch(&mut buf, &mut scratch);
        let power: [f32; 201] = std::array::from_fn(|k| buf[k].norm_sqr());
        for (b, weights) in bands.iter().enumerate() {
            let v = weights
                .iter()
                .map(|&(k, w)| w * power[k])
                .sum::<f32>()
                .max(1e-10)
                .log10();
            out[b * 800 + t] = v;
            peak = peak.max(v);
        }
    }
    out.iter_mut()
        .for_each(|v| *v = (v.max(peak - 8.) + 4.) / 4.);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pause_score_cannot_end_resumed_speech_or_the_next_turn() {
        let (jobs, requests) = mpsc::sync_channel(1);
        let (results, scores) = mpsc::channel();
        let mut detector = SmartTurn {
            frames: VecDeque::new(),
            jobs,
            scores,
            generation: 0,
            asked: false,
            voiced_frames: 0,
            confirmed: false,
        };
        for _ in 0..44 {
            detector.push([0; 320], true, 0);
        }
        for n in 1..125 {
            assert!(!detector.push([0; 320], false, n));
        }
        assert!(
            requests.try_recv().is_err(),
            "a short opener keeps its thinking pause"
        );
        detector.push([0; 320], true, 0);
        for n in 1..40 {
            assert!(!detector.push([0; 320], false, n));
        }
        let (first, _) = requests.try_recv().unwrap();
        results.send((first, 0.99)).unwrap();
        assert!(!detector.push([0; 320], true, 0));
        for n in 1..40 {
            assert!(!detector.push([0; 320], false, n));
        }
        let (second, _) = requests.try_recv().unwrap();
        results.send((second, 0.99)).unwrap();
        for n in 40..80 {
            assert!(!detector.push([0; 320], false, n));
        }
        assert!(detector.push([0; 320], false, 80));
        detector.clear();
        results.send((second, 1.)).unwrap();
        assert!(!detector.push([0; 320], false, 41));
    }
}
