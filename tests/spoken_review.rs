// Run via tools/spoken_review_checks.sh, which creates the disposable project and audio.
#![cfg(target_os = "macos")]

use serde_json::Value;
use std::{env, fs, path::PathBuf, process::Command};

fn root() -> PathBuf {
    env::var_os("HEY_BOSS_FLEET_STATE").unwrap().into()
}

fn cli() -> Command {
    let mut cmd = Command::new(env::var_os("HEY_BOSS_CLI_PATH").unwrap());
    cmd.current_dir(root());
    cmd
}

fn pcm(bytes: &[u8]) -> Vec<i16> {
    assert!(bytes.len().is_multiple_of(2), "partial PCM sample");
    bytes
        .chunks_exact(2)
        .map(|v| i16::from_le_bytes([v[0], v[1]]))
        .collect()
}

fn detect(input: &[i16], args: &[&str]) -> (Vec<Vec<i16>>, String) {
    let file = root().join("detector-input.pcm");
    fs::write(
        &file,
        input
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let output = cli()
        .arg("review-audio")
        .args(args)
        .stdin(fs::File::open(file).unwrap())
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "{log}");
    let mut bytes = output.stdout.as_slice();
    let mut turns = Vec::new();
    let mut turn: Option<Vec<i16>> = None;
    let mut bookmarked = false;
    let mut continued = false;
    while !bytes.is_empty() {
        let (header, rest) = bytes.split_at(2);
        bytes = rest;
        match u16::from_le_bytes(header.try_into().unwrap()) {
            2 if turn.is_some() => {
                assert!(
                    args.contains(&"--conversation"),
                    "unexpected sustained speech marker"
                );
                continued = true;
            }
            2 => bookmarked = true,
            0 => {
                assert!(
                    bookmarked && turn.is_none(),
                    "missing bookmark or nested turn"
                );
                turn = Some(Vec::new());
                bookmarked = false;
            }
            1 => {
                assert!(
                    !args.contains(&"--conversation") || continued,
                    "sustained speech not confirmed"
                );
                turns.push(turn.take().expect("ended outside a turn"));
                continued = false;
            }
            640 => {
                let (frame, rest) = bytes.split_at(640);
                turn.as_mut()
                    .expect("audio outside a turn")
                    .extend(pcm(frame));
                bytes = rest;
            }
            other => panic!("unexpected frame size {other}"),
        }
    }
    assert!(turn.is_none(), "unterminated turn");
    (turns, log)
}

fn silence(ms: usize) -> Vec<i16> {
    vec![0; ms * 16]
}

fn at(input: &[i16], clip: &[i16]) -> usize {
    input
        .windows(clip.len())
        .position(|w| w == clip)
        .expect("turn absent from input")
}

#[test]
#[ignore = "requires generated audio and the Mac Ping sound"]
fn interruption_and_endpoint_timing() {
    let voice = pcm(&fs::read(root().join("voice.pcm")).unwrap());
    let second = pcm(&fs::read(root().join("second.pcm")).unwrap());
    let quiet: Vec<i16> = voice
        .iter()
        .map(|&s| (f64::from(s) / 10.).round() as i16)
        .collect();
    let mut seed = 12345_i64;
    let noise: Vec<i16> = (0..48000)
        .map(|_| {
            seed = seed * 16807 % 2147483647;
            ((seed as f64 / 2147483647. * 2. - 1.) * 1000.).round() as i16
        })
        .collect();
    let click: Vec<i16> = (0..16000)
        .map(|i| {
            if (3001..3070).contains(&i) {
                (20000. * (i as f64).sin()).round() as i16
            } else {
                0
            }
        })
        .collect();
    let input = [
        &silence(1000)[..],
        &noise,
        &silence(1000),
        &click,
        &silence(1500),
    ]
    .concat();
    assert!(
        detect(&input, &[]).0.is_empty(),
        "background noise or click opened a turn"
    );

    let typing: Vec<i16> = (0..64000)
        .map(|i| {
            if i % 2400 < 160 {
                ((-(i as f64 % 2400.) / 45.).exp() * f64::from(noise[i % noise.len()]) * 18.)
                    .round() as i16
            } else {
                0
            }
        })
        .collect();
    let chat = &["--conversation"];
    let (_, log) = detect(&[&silence(2000)[..], &voice, &silence(3500)].concat(), chat);
    let confirmed: usize = log
        .split_once("speech confirmed at ")
        .unwrap()
        .1
        .split_once("ms")
        .unwrap()
        .0
        .parse()
        .unwrap();
    let peak = f64::from(voice.iter().map(|v| v.unsigned_abs()).max().unwrap());
    let onset = voice
        .iter()
        .position(|v| f64::from(v.unsigned_abs()) >= peak * 0.015)
        .unwrap();
    assert!(
        confirmed <= 2000 + onset / 16 + 250,
        "late confirmation: {confirmed} ms"
    );

    let ping = Command::new("ffmpeg")
        .args([
            "-loglevel",
            "error",
            "-i",
            "/System/Library/Sounds/Ping.aiff",
            "-ar",
            "16000",
            "-ac",
            "1",
            "-f",
            "s16le",
            "-",
        ])
        .output()
        .unwrap();
    assert!(
        ping.status.success(),
        "{}",
        String::from_utf8_lossy(&ping.stderr)
    );
    for sound in [&pcm(&ping.stdout), &typing] {
        let input = [&silence(1000)[..], sound, &silence(3500)].concat();
        assert!(
            detect(&input, chat).0.is_empty(),
            "notification or typing opened a chat turn"
        );
    }
    for speech in [&voice, &quiet] {
        let mixed: Vec<_> = speech
            .iter()
            .enumerate()
            .map(|(i, s)| s.saturating_add(typing[i % typing.len()]))
            .collect();
        let input = [&silence(1000)[..], &typing, &mixed, &typing, &silence(3500)].concat();
        let (turns, _) = detect(&input, chat);
        let [turn] = turns.as_slice() else {
            panic!("expected one turn amid typing")
        };
        assert!(
            at(&input, turn) + turn.len() < (1000 + 3500) * 16 + typing.len() + mixed.len(),
            "typing held the turn open"
        );

        let input = [&silence(1000)[..], speech, &silence(1500)].concat();
        let (turns, _) = detect(&input, &[]);
        let [turn] = turns.as_slice() else {
            panic!("expected one spoken comment")
        };
        let start = at(&input, turn);
        assert!(
            (600 * 16..1000 * 16).contains(&start),
            "pre-roll lost the first word"
        );
        let latency = start - 600 * 16;
        let tail = (start + turn.len())
            .checked_sub(1000 * 16 + speech.len())
            .expect("clipped speech");
        assert!((100 * 16..=320 * 16).contains(&latency));
        assert!((650 * 16..=1150 * 16).contains(&tail));
        println!(
            "Interruption: {} ms; endpoint: {} ms",
            latency / 16,
            (tail + 8) / 16
        );
    }
    for (gap, count) in [(300, 1), (500, 1), (1300, 2)] {
        let input = [
            &silence(1000)[..],
            &voice,
            &silence(gap),
            &voice,
            &silence(1500),
        ]
        .concat();
        assert_eq!(detect(&input, &[]).0.len(), count, "pause of {gap} ms");
    }

    let first = [&silence(1000)[..], &voice, &silence(3600)].concat();
    let (turns, _) = detect(&first, chat);
    let [alone] = turns.as_slice() else {
        panic!("expected one speaker")
    };
    let end = at(&first, alone) + alone.len() + 320; // endpoint frame is not emitted
    let input = [&first[..end - 60 * 16], &second, &silence(3400)].concat();
    let (turns, _) = detect(&input, chat);
    assert_eq!(turns.len(), 2);
    at(&turns[1], &second); // keep the next speaker's whole opening word

    for interrupt in [false, true] {
        let playback = |i: i32| {
            if (16000..96000).contains(&i) {
                voice[(i as usize - 16000) % voice.len()]
            } else {
                0
            }
        };
        let stereo: Vec<_> = (0..8 * 16000)
            .flat_map(|i| {
                let near = if interrupt && i >= 40000 {
                    second.get((i - 40000) as usize).copied().unwrap_or(0)
                } else {
                    0
                };
                [
                    (f64::from(playback(i - 1440)) * 0.08 + f64::from(near) * 0.15).round() as i16,
                    playback(i),
                ]
            })
            .collect();
        for args in [&["--duplex"][..], &["--duplex", "--conversation"]] {
            assert_eq!(
                detect(&stereo, args).0.len(),
                usize::from(interrupt),
                "echo and overlapping speech: {args:?}"
            );
        }
    }
}

#[test]
#[ignore = "requires the Mac desktop audit binary and an isolated owner"]
fn saved_review_comments() {
    let artifact = |args: &[&str]| {
        let output = cli()
            .args(["artifact", "--project", "named:SpokenReview", "--json"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    let created = artifact(&[
        "create",
        "--title",
        "Review fixture",
        "--body",
        "# First heading\n\nFirst passage.\nSecond passage has several words and continues long enough to interrupt and resume reading.\n\nFinal passage. This section addresses a separate point in the review.",
    ]);
    let id = created["artifact"]["id"].as_str().unwrap();
    let status = Command::new(env::var_os("HEY_BOSS_REVIEW_AUDIT").unwrap())
        .current_dir(root())
        .env("HEY_BOSS_AUDIT_SPOKEN_REVIEW", id)
        .status()
        .unwrap();
    assert!(status.success(), "desktop audit: {status}");
    let view = artifact(&["view", id]);
    let [c, second, recovered, typed, reply, note, title] =
        view["comments"].as_array().unwrap().as_slice()
    else {
        panic!("expected seven saved comments")
    };
    assert_eq!(title["body"], "Shorten the title.");
    assert_eq!(title.get("quote"), Some(&Value::Null));
    assert_eq!(title.get("parent"), Some(&Value::Null));
    assert_eq!(typed["quote"], "First passage.");
    assert_eq!(typed["outdated"], false);
    assert_eq!(typed["resolved"], true);
    assert_eq!(reply["parent"], typed["id"]);
    assert_eq!(note.get("quote"), Some(&Value::Null));
    assert_eq!(note.get("parent"), Some(&Value::Null));
    for (comment, quote, line, words) in [
        (second, "Final passage.", 6, "date to the last example"),
        (
            c,
            "Second passage",
            4,
            "move this paragraph after the introduction",
        ),
    ] {
        assert_eq!(comment["outdated"], false);
        assert!(comment["quote"].as_str().unwrap().starts_with(quote));
        let body = comment["body"].as_str().unwrap();
        assert!(
            body.starts_with(&format!("Line {line} · revision 1"))
                && body.to_lowercase().contains(words),
            "{body}"
        );
    }
    assert!(
        recovered["body"]
            .as_str()
            .unwrap()
            .ends_with("Keep this quote.")
    );
    assert_eq!(recovered["outdated"], false);
}
