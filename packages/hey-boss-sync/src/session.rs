//! One bounded exchange over an authenticated byte stream (SSH stdio).
use crate::{Batch, Engine, Result, invalid, wire};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Hello {
    protocol: u32,
    replica: String,
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
        protocol: 1,
        replica: engine.id().into(),
    }
}
fn validate(engine: &Engine, peer: &Hello) -> Result<()> {
    if peer.protocol != 1
        || peer.replica.len() != 32
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
    let peer: Hello = required(reader)?;
    validate(engine, &peer)?;
    wire::send(
        writer,
        &Welcome {
            hello: hello(engine),
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
    engine.ingest(&turn.batch)?;
    wire::send(
        writer,
        &Turn {
            received: engine.received(&peer.replica)?,
            batch,
        },
    )?;
    let acknowledged: i64 = required(reader)?;
    if acknowledged != through {
        return Err(invalid("SSH peer acknowledged a different batch"));
    }
    engine.acknowledge(&peer.replica, acknowledged)?;
    Ok(())
}

pub fn connect(engine: &mut Engine, reader: &mut impl Read, writer: &mut impl Write) -> Result<()> {
    wire::send(writer, &hello(engine))?;
    let peer: Welcome = required(reader)?;
    validate(engine, &peer.hello)?;
    let sent = engine.batch(peer.received)?;
    let through = sent.through;
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
    wire::send(writer, &engine.received(&peer.hello.replica)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

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
