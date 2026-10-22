//! One bounded exchange over an authenticated byte stream (SSH stdio).
use crate::{Batch, Engine, Result, invalid, wire};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    time::{Duration, Instant},
};

/// Replaceable liveness evidence, deliberately outside the immutable fact log.
/// Relays forward the remaining lifetime, never restart it. The origin's durable
/// sequence makes a replay harmless even after a process or machine restart.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Presence {
    pub machine: String,
    pub sequence: i64,
    pub attempts: Vec<String>,
    pub remaining_ms: i64,
}

pub struct Exchange {
    pub records: usize,
    pub peer: String,
    /// An empty incoming page proves the sender had no more facts at capture.
    /// A partial page alone cannot prove this: byte limits also bound pages.
    pub incoming_drained: bool,
    pub outgoing_pending: i64,
    pub journal_head: i64,
    pub presence: Vec<Presence>,
    /// Conservative age covering both directions, without comparing wall clocks.
    pub presence_age: Duration,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum Acknowledgment {
    Timed { received: i64, elapsed_ms: u64 },
    Legacy(i64),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Hello {
    protocol: u32,
    replica: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    presence: Vec<Presence>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Welcome {
    hello: Hello,
    received: i64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Turn {
    received: i64,
    batch: Batch,
}

fn hello(engine: &Engine) -> Hello {
    Hello {
        // Queue rotations require an upgraded application projector. Fence
        // older peers before either side sends or acknowledges durable facts.
        // The immutable journal's payload format and history are unchanged.
        protocol: 2,
        replica: engine.id().into(),
        presence: Vec::new(),
    }
}
fn validate(engine: &Engine, peer: &Hello) -> Result<()> {
    if peer.protocol != 2 {
        return Err(invalid(
            "Queue ordering requires upgraded sync peers; run hey-boss upgrade on the supervisor and reconnect both peers",
        ));
    }
    if peer.replica.len() != 32
        || !peer.replica.bytes().all(|b| b.is_ascii_hexdigit())
        || peer.replica == engine.id()
    {
        return Err(invalid(
            "Incompatible sync protocol or duplicated replica identity",
        ));
    }
    Ok(())
}
fn required<T: serde::de::DeserializeOwned>(reader: &mut impl Read) -> Result<T> {
    wire::receive(reader)?.ok_or_else(|| invalid("SSH synchronization ended before acknowledgment"))
}

pub fn serve(engine: &mut Engine, reader: &mut impl Read, writer: &mut impl Write) -> Result<()> {
    serve_next(engine, reader, writer, |_| Ok(Vec::new()))?
        .ok_or_else(|| invalid("SSH synchronization ended before handshake"))?;
    Ok(())
}

/// Wait without holding a database transaction, then prepare current facts only
/// after the request arrives. EOF between exchanges is a clean shutdown.
pub fn serve_next(
    engine: &mut Engine,
    reader: &mut impl Read,
    writer: &mut impl Write,
    prepare: impl FnOnce(&mut Engine) -> Result<Vec<Presence>>,
) -> Result<Option<Exchange>> {
    let Some(peer): Option<Hello> = wire::receive(reader)? else {
        return Ok(None);
    };
    validate(engine, &peer)?;
    let started = Instant::now();
    let mut greeting = hello(engine);
    greeting.presence = prepare(engine)?;
    wire::send(
        writer,
        &Welcome {
            hello: greeting,
            received: engine.received(&peer.replica)?,
        },
    )?;
    let turn: Turn = required(reader)?;
    if turn.batch.replica != peer.replica {
        return Err(invalid("SSH peer changed replica identity"));
    }
    // Validate the requested cursor before committing any incoming mutation.
    let batch = engine.batch(turn.received)?;
    let through = batch.through;
    let records = batch.records.len() + turn.batch.records.len();
    engine.ingest(&turn.batch)?;
    wire::send(
        writer,
        &Turn {
            received: engine.received(&peer.replica)?,
            batch,
        },
    )?;
    let acknowledgment: Acknowledgment = required(reader)?;
    let (acknowledged, peer_elapsed) = match acknowledgment {
        Acknowledgment::Timed {
            received,
            elapsed_ms,
        } => (received, Duration::from_millis(elapsed_ms)),
        Acknowledgment::Legacy(received) => (received, Duration::ZERO),
    };
    if acknowledged != through {
        return Err(invalid("SSH peer acknowledged a different batch"));
    }
    engine.acknowledge(&peer.replica, acknowledged)?;
    let journal_head = engine.head()?;
    Ok(Some(Exchange {
        records,
        peer: peer.replica,
        incoming_drained: turn.batch.records.is_empty(),
        outgoing_pending: journal_head.saturating_sub(acknowledged),
        journal_head,
        presence: peer.presence,
        presence_age: peer_elapsed.saturating_add(started.elapsed()),
    }))
}

pub fn connect(engine: &mut Engine, reader: &mut impl Read, writer: &mut impl Write) -> Result<()> {
    connect_present(engine, reader, writer, Vec::new())?;
    Ok(())
}
pub fn connect_present(
    engine: &mut Engine,
    reader: &mut impl Read,
    writer: &mut impl Write,
    presence: Vec<Presence>,
) -> Result<Exchange> {
    let started = Instant::now();
    let timed = !presence.is_empty();
    let mut greeting = hello(engine);
    greeting.presence = presence;
    wire::send(writer, &greeting)?;
    let peer: Welcome = required(reader)?;
    validate(engine, &peer.hello)?;
    let sent = engine.batch(peer.received)?;
    let through = sent.through;
    let sent_count = sent.records.len();
    wire::send(
        writer,
        &Turn {
            received: engine.received(&peer.hello.replica)?,
            batch: sent,
        },
    )?;
    let turn: Turn = required(reader)?;
    if turn.batch.replica != peer.hello.replica || turn.received != through {
        return Err(invalid("SSH peer acknowledged a different batch"));
    }
    engine.ingest(&turn.batch)?;
    engine.acknowledge(&peer.hello.replica, turn.received)?;
    let received = engine.received(&peer.hello.replica)?;
    let acknowledgment = if timed {
        Acknowledgment::Timed {
            received,
            elapsed_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        }
    } else {
        Acknowledgment::Legacy(received)
    };
    wire::send(writer, &acknowledgment)?;
    let journal_head = engine.head()?;
    Ok(Exchange {
        records: sent_count + turn.batch.records.len(),
        peer: peer.hello.replica,
        incoming_drained: turn.batch.records.is_empty(),
        outgoing_pending: journal_head.saturating_sub(turn.received),
        journal_head,
        presence: peer.hello.presence,
        presence_age: started.elapsed(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    #[test]
    fn old_peer_is_rejected_before_preparation_or_journal_exchange() {
        let root = tempfile::tempdir().unwrap();
        let mut engine = Engine::open(&root.path().join("sync.db")).unwrap();
        engine.put("existing immutable history").unwrap();
        let mut frame = Vec::new();
        wire::send(
            &mut frame,
            &Hello {
                protocol: 1,
                replica: "a".repeat(32),
                presence: Vec::new(),
            },
        )
        .unwrap();
        let mut output = Vec::new();
        let result = serve_next(&mut engine, &mut frame.as_slice(), &mut output, |_| {
            panic!("must reject before preparation")
        });
        assert!(result.err().unwrap().to_string().contains("upgrade"));
        assert!(output.is_empty());
        assert_eq!(engine.head().unwrap(), 1);
        assert_eq!(engine.received(&"a".repeat(32)).unwrap(), 0);
    }

    #[test]
    fn persistent_stream_refreshes_presence_without_growing_the_fact_log() {
        let root = tempfile::tempdir().unwrap();
        let mut a = Engine::open(&root.path().join("a.db")).unwrap();
        let mut b = Engine::open(&root.path().join("b.db")).unwrap();
        a.put("reservation-a").unwrap();
        b.put("reservation-b").unwrap();
        let (mut left, mut right) = UnixStream::pair().unwrap();
        for socket in [&left, &right] {
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
        }
        std::thread::scope(|scope| {
            let server = scope.spawn(|| {
                for n in 1..=10 {
                    let result =
                        serve_next(&mut b, &mut right.try_clone().unwrap(), &mut right, |_| {
                            Ok(vec![Presence {
                                machine: "b".into(),
                                sequence: n,
                                attempts: vec!["b1".into()],
                                remaining_ms: 100,
                            }])
                        })
                        .unwrap()
                        .unwrap();
                    assert_eq!(result.presence[0].sequence, n);
                }
                assert!(
                    serve_next(
                        &mut b,
                        &mut right.try_clone().unwrap(),
                        &mut right,
                        |_| panic!("EOF must not prepare or write")
                    )
                    .unwrap()
                    .is_none()
                );
            });
            for n in 1..=10 {
                let result = connect_present(
                    &mut a,
                    &mut left.try_clone().unwrap(),
                    &mut left,
                    vec![Presence {
                        machine: "a".into(),
                        sequence: n,
                        attempts: vec!["a1".into()],
                        remaining_ms: 100,
                    }],
                )
                .unwrap();
                assert_eq!(result.presence[0].sequence, n);
                if n > 2 {
                    assert_eq!(result.records, 0);
                }
            }
            drop(left);
            server.join().unwrap();
        });
        assert_eq!(a.head().unwrap(), 2);
        assert_eq!(b.head().unwrap(), 2);
    }

    #[test]
    fn delayed_initial_message_is_included_in_received_presence_age() {
        let root = tempfile::tempdir().unwrap();
        let mut a = Engine::open(&root.path().join("a.db")).unwrap();
        let mut b = Engine::open(&root.path().join("b.db")).unwrap();
        let (mut left, mut right) = UnixStream::pair().unwrap();
        let delay = Duration::from_millis(30);
        std::thread::scope(|scope| {
            let client = scope.spawn(|| {
                connect_present(
                    &mut a,
                    &mut left.try_clone().unwrap(),
                    &mut left,
                    vec![Presence {
                        machine: "a".into(),
                        sequence: 1,
                        attempts: vec!["run".into()],
                        remaining_ms: 100,
                    }],
                )
            });
            // Read the greeting before delaying delivery to serve_next. This
            // makes the delay causally follow the sender's presence capture.
            let greeting: Hello = required(&mut right).unwrap();
            std::thread::sleep(delay);
            let mut frame = Vec::new();
            wire::send(&mut frame, &greeting).unwrap();
            let mut reader = std::io::Cursor::new(frame).chain(right.try_clone().unwrap());
            let result = serve_next(&mut b, &mut reader, &mut right, |_| Ok(Vec::new()))
                .unwrap()
                .unwrap();
            assert!(
                result.presence_age >= delay,
                "Receiver must deduct delay before it read Hello"
            );
            client.join().unwrap().unwrap();
        });
    }

    fn exchange(a: &mut Engine, b: &mut Engine) {
        let (mut left, mut right) = UnixStream::pair().unwrap();
        left.set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        right
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        std::thread::scope(|scope| {
            let server = scope.spawn(|| serve(b, &mut right.try_clone().unwrap(), &mut right));
            connect(a, &mut left.try_clone().unwrap(), &mut left).unwrap();
            server.join().unwrap().unwrap();
        });
    }

    #[test]
    fn an_applied_page_does_not_prove_the_peer_is_drained() {
        let root = tempfile::tempdir().unwrap();
        let mut a = Engine::open(&root.path().join("a.db")).unwrap();
        let mut b = Engine::open(&root.path().join("b.db")).unwrap();
        for n in 0..300 {
            b.put(&format!("record-{n}")).unwrap();
        }
        let peer = b.id().to_owned();
        let (mut left, mut right) = UnixStream::pair().unwrap();
        std::thread::scope(|scope| {
            let server = scope.spawn(|| {
                for _ in 0..5 {
                    serve(&mut b, &mut right.try_clone().unwrap(), &mut right).unwrap();
                }
            });
            for n in 0..5 {
                let result = connect_present(
                    &mut a,
                    &mut left.try_clone().unwrap(),
                    &mut left,
                    Vec::new(),
                )
                .unwrap();
                assert_eq!(result.peer, peer);
                assert_eq!(result.incoming_drained, n >= 3);
                if n == 0 {
                    assert_eq!(result.outgoing_pending, 128);
                }
            }
            server.join().unwrap();
        });
    }

    #[test]
    fn lost_final_acknowledgment_replays_without_duplicate_records_after_restart() {
        let root = tempfile::tempdir().unwrap();
        let mut a = Engine::open(&root.path().join("a.db")).unwrap();
        let mut b = Engine::open(&root.path().join("b.db")).unwrap();
        a.put("offline-a").unwrap();
        b.put("offline-b").unwrap();
        let (mut left, mut right) = UnixStream::pair().unwrap();
        std::thread::scope(|scope| {
            let server = scope.spawn(|| serve(&mut b, &mut right.try_clone().unwrap(), &mut right));
            wire::send(&mut left, &hello(&a)).unwrap();
            let peer: Welcome = required(&mut left).unwrap();
            wire::send(
                &mut left,
                &Turn {
                    received: 0,
                    batch: a.batch(peer.received).unwrap(),
                },
            )
            .unwrap();
            let turn: Turn = required(&mut left).unwrap();
            a.ingest(&turn.batch).unwrap();
            drop(left); // Both durable merges happened; the last receipt did not.
            assert!(server.join().unwrap().is_err());
        });
        drop(a);
        drop(b);
        let mut a = Engine::open(&root.path().join("a.db")).unwrap();
        let mut b = Engine::open(&root.path().join("b.db")).unwrap();
        for _ in 0..3 {
            exchange(&mut a, &mut b);
        }
        assert_eq!(a.head().unwrap(), 2);
        assert_eq!(b.head().unwrap(), 2);
        assert_eq!(a.received(b.id()).unwrap(), 2);
        assert_eq!(b.received(a.id()).unwrap(), 2);
    }

    #[test]
    fn ssh_stdio_relay_recovers_both_disconnected_origins_and_restarts() {
        let root = tempfile::tempdir().unwrap();
        let mut a = Engine::open(&root.path().join("a.db")).unwrap();
        let mut b = Engine::open(&root.path().join("b.db")).unwrap();
        let mut relay = Engine::open(&root.path().join("relay.db")).unwrap();
        for n in 0..300 {
            a.put(&format!("attempt-a-{n}")).unwrap();
            b.put(&format!("attempt-b-{n}")).unwrap();
        }
        for _ in 0..8 {
            exchange(&mut relay, &mut a);
            exchange(&mut relay, &mut b);
        }
        drop(relay);
        let mut relay = Engine::open(&root.path().join("relay.db")).unwrap();
        for _ in 0..8 {
            exchange(&mut relay, &mut b);
            exchange(&mut relay, &mut a);
        }
        assert_eq!(a.head().unwrap(), 600);
        assert_eq!(b.head().unwrap(), 600);
        assert_eq!(a.head().unwrap(), relay.head().unwrap());
        assert_eq!(a.received(relay.id()).unwrap(), 600);
        assert_eq!(b.received(relay.id()).unwrap(), 600);
    }
}
