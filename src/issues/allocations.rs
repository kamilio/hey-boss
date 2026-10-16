//! Replicated, single-owner reservation facts; local monotonic leases and a
//! deterministic projection supply the existing machine-allocation interface.
//! A remote fact never controls a process. Its local owner observes a lost
//! allocation through the normal cancellation path, preserving its checkout.
use super::{Error, Result};
use crate::database::Connection;
use rusqlite::{OptionalExtension, params};
use std::time::Duration;

/// One scheduling budget, shared with replication. Two hops cover the relay
/// topology; two traversals let a reservation and a competing launch propagate.
pub(crate) const SYNC_INTERVAL: Duration = Duration::from_secs(2);
const RELAY_HOPS: u32 = 2;
const SETTLE_TRAVERSALS: u32 = 2;
const MISSED_RENEWALS: u32 = 3;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Timing {
    pub settle: Duration,
    pub lease: Duration,
}
impl Timing {
    pub fn from_exchange(exchange: Duration) -> Self {
        let settle = (SYNC_INTERVAL + exchange) * RELAY_HOPS * SETTLE_TRAVERSALS;
        Self {
            settle,
            lease: settle * MISSED_RENEWALS,
        }
    }
}
fn ms(value: Duration) -> i64 {
    value.as_millis().min(i64::MAX as u128) as i64
}

/// Comparable only on this boot of this machine. No remote wall-clock timestamp
/// participates in lease expiry, launch ordering, or the settling countdown.
#[derive(Clone, Debug)]
pub(crate) struct Clock {
    pub boot: String,
    pub tick: i64,
}
impl Clock {
    pub fn read() -> Result<Self> {
        let mut time = std::mem::MaybeUninit::<libc::timespec>::uninit();
        #[cfg(target_os = "linux")]
        let clock = libc::CLOCK_BOOTTIME;
        #[cfg(not(target_os = "linux"))]
        let clock = libc::CLOCK_MONOTONIC;
        if unsafe { libc::clock_gettime(clock, time.as_mut_ptr()) } != 0 {
            return Err(Error::new(
                "allocation_clock",
                std::io::Error::last_os_error().to_string(),
            ));
        }
        let time = unsafe { time.assume_init() };
        static BOOT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        let boot = if let Some(boot) = BOOT.get() {
            boot.clone()
        } else {
            #[cfg(target_os = "macos")]
            let boot = {
                // Unlike kern.boottime, this does not change when the wall clock
                // is corrected. Every process on the same boot reads the same ID.
                let mut value = [0_u8; 64];
                let mut len = value.len();
                if unsafe {
                    libc::sysctlbyname(
                        c"kern.bootsessionuuid".as_ptr(),
                        value.as_mut_ptr().cast(),
                        &mut len,
                        std::ptr::null_mut(),
                        0,
                    )
                } != 0
                    || len == 0
                    || len > value.len()
                {
                    return Err(Error::new(
                        "allocation_clock",
                        "Cannot identify this system boot",
                    ));
                }
                String::from_utf8_lossy(&value[..len])
                    .trim_end_matches('\0')
                    .to_owned()
            };
            #[cfg(not(target_os = "macos"))]
            let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?
                .trim()
                .to_owned();
            let _ = BOOT.set(boot.clone());
            boot
        };
        Ok(Self {
            boot,
            tick: time
                .tv_sec
                .saturating_mul(1000)
                .saturating_add(time.tv_nsec / 1_000_000),
        })
    }
}

pub(crate) fn migrate(db: &Connection) -> Result<()> {
    if db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='worker_allocation_release')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(());
    }
    let timing = Timing::from_exchange(Duration::ZERO);
    let tx = db.unchecked_transaction()?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS worker_allocations(
        id TEXT PRIMARY KEY,project_id TEXT NOT NULL,issue_number INTEGER NOT NULL,machine TEXT NOT NULL,
        ticket INTEGER NOT NULL,launched INTEGER,
        released INTEGER NOT NULL DEFAULT 0,settle_ms INTEGER NOT NULL,lease_ms INTEGER NOT NULL,presence_after INTEGER NOT NULL DEFAULT 0);
        CREATE INDEX IF NOT EXISTS worker_allocations_active ON worker_allocations(project_id,issue_number,id) WHERE released=0;
        CREATE INDEX IF NOT EXISTS worker_allocations_machine ON worker_allocations(machine,id) WHERE released=0;
        CREATE TABLE IF NOT EXISTS worker_allocation_observations(id TEXT PRIMARY KEY,boot TEXT NOT NULL,deadline_tick INTEGER NOT NULL,ready_tick INTEGER NOT NULL,presence_sequence INTEGER NOT NULL DEFAULT -1);
        CREATE TABLE IF NOT EXISTS worker_allocation_winners(project_id TEXT NOT NULL,issue_number INTEGER NOT NULL,id TEXT NOT NULL,machine TEXT NOT NULL,PRIMARY KEY(project_id,issue_number));
        CREATE TABLE IF NOT EXISTS worker_allocation_clock(id INTEGER PRIMARY KEY CHECK(id=1),clock INTEGER NOT NULL DEFAULT 0,boot TEXT NOT NULL DEFAULT '',renewed_tick INTEGER NOT NULL DEFAULT 0,settle_ms INTEGER NOT NULL,lease_ms INTEGER NOT NULL,presence_sequence INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE IF NOT EXISTS worker_allocation_presence(machine TEXT PRIMARY KEY,sequence INTEGER NOT NULL,attempts TEXT NOT NULL,boot TEXT NOT NULL,seen_tick INTEGER NOT NULL,remaining_ms INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS distributed_sync_timings(peer TEXT PRIMARY KEY,exchange_ms INTEGER NOT NULL);")?;
    tx.execute(
        "INSERT OR IGNORE INTO worker_allocation_clock(id,settle_ms,lease_ms) VALUES(1,?1,?2)",
        params![ms(timing.settle), ms(timing.lease)],
    )?;
    tx.execute_batch("CREATE TRIGGER IF NOT EXISTS worker_allocation_reserve AFTER INSERT ON worker_runs
        WHEN (SELECT role FROM fleet_meta WHERE id=1)='peer'
        BEGIN
        UPDATE worker_allocation_clock SET clock=clock+1 WHERE id=1;
        INSERT INTO worker_allocations(id,project_id,issue_number,machine,ticket,settle_ms,lease_ms,presence_after)
        SELECT NEW.id,NEW.project_id,NEW.issue_number,NEW.machine,clock,settle_ms,lease_ms,presence_sequence+1 FROM worker_allocation_clock WHERE id=1;
        END;
        CREATE TRIGGER IF NOT EXISTS worker_allocation_release AFTER UPDATE OF finished_at ON worker_runs
        WHEN NEW.finished_at IS NOT NULL AND OLD.finished_at IS NULL
        BEGIN UPDATE worker_allocations SET released=1 WHERE id=NEW.id AND released=0; END;")?;
    tx.commit()?;
    Ok(())
}

pub(crate) fn enabled(db: &Connection) -> Result<bool> {
    Ok(
        db.query_row("SELECT role='peer' FROM fleet_meta WHERE id=1", [], |r| {
            r.get(0)
        })?,
    )
}

pub(crate) fn record_exchange(db: &Connection, peer: &str, elapsed: Duration) -> Result<()> {
    // A decaying high-water mark reacts immediately to a slower exchange without
    // retaining a historical outage forever. No per-host tuning parameters.
    db.execute("INSERT INTO distributed_sync_timings VALUES(?1,?2) ON CONFLICT(peer) DO UPDATE SET exchange_ms=max(excluded.exchange_ms,(exchange_ms+excluded.exchange_ms)/2)", params![peer, ms(elapsed)])?;
    Ok(())
}

/// Called inside the application's write transaction after projection or pickup.
/// Unchanged/replayed heartbeats never extend a lease. Each replica ages what it
/// actually received, so skewed clocks and stale replay cannot keep claims alive.
pub(crate) fn reconcile(db: &Connection, clock: &Clock) -> Result<()> {
    if !enabled(db)? {
        return Ok(());
    }
    db.execute("INSERT INTO worker_allocation_observations(id,boot,deadline_tick,ready_tick)
        SELECT id,?1,?2+lease_ms,?2+settle_ms FROM worker_allocations WHERE released=0
        ON CONFLICT(id) DO UPDATE SET boot=excluded.boot,
        deadline_tick=CASE WHEN worker_allocation_observations.boot<>excluded.boot THEN ?2 ELSE worker_allocation_observations.deadline_tick END,
        presence_sequence=CASE WHEN worker_allocation_observations.boot<>excluded.boot THEN -1 ELSE worker_allocation_observations.presence_sequence END,
        ready_tick=CASE WHEN worker_allocation_observations.boot<>excluded.boot THEN excluded.ready_tick ELSE worker_allocation_observations.ready_tick END
        WHERE excluded.boot<>worker_allocation_observations.boot", params![clock.boot, clock.tick])?;
    db.execute("UPDATE worker_allocation_observations AS o SET
        presence_sequence=p.sequence,
        deadline_tick=CASE WHEN EXISTS(SELECT 1 FROM json_each(p.attempts) WHERE value=a.id) THEN p.seen_tick+p.remaining_ms ELSE ?2 END
        FROM worker_allocations a JOIN worker_allocation_presence p ON p.machine=a.machine
        WHERE o.id=a.id AND a.released=0 AND p.boot=?1 AND p.sequence>=a.presence_after AND p.sequence>o.presence_sequence",params![clock.boot,clock.tick])?;
    // Work on the active frontier only. Historical attempt facts remain durable.
    db.execute("DELETE FROM fleet_allocations WHERE (project_id,issue_number) IN (
        SELECT w.project_id,w.issue_number FROM worker_allocation_winners w WHERE NOT EXISTS(
        SELECT 1 FROM worker_allocations a JOIN worker_allocation_observations o ON o.id=a.id
        WHERE a.project_id=w.project_id AND a.issue_number=w.issue_number AND a.released=0 AND o.boot=?1 AND o.deadline_tick>?2))", params![clock.boot,clock.tick])?;
    db.execute("DELETE FROM worker_allocation_winners AS w WHERE NOT EXISTS(
        SELECT 1 FROM worker_allocations a JOIN worker_allocation_observations o ON o.id=a.id
        WHERE a.project_id=w.project_id AND a.issue_number=w.issue_number AND a.released=0 AND o.boot=?1 AND o.deadline_tick>?2)",params![clock.boot,clock.tick])?;
    db.execute("INSERT INTO worker_allocation_winners
        SELECT project_id,issue_number,id,machine FROM (
        SELECT a.project_id,a.issue_number,a.id,a.machine,
          row_number() OVER (PARTITION BY a.project_id,a.issue_number ORDER BY a.launched IS NULL,coalesce(a.launched,a.ticket),a.machine,a.id) AS rank
        FROM worker_allocations a JOIN worker_allocation_observations o ON o.id=a.id
        WHERE a.released=0 AND o.boot=?1 AND o.deadline_tick>?2) WHERE rank=1
        ON CONFLICT(project_id,issue_number) DO UPDATE SET id=excluded.id,machine=excluded.machine
        WHERE worker_allocation_winners.id<>excluded.id OR worker_allocation_winners.machine<>excluded.machine", params![clock.boot,clock.tick])?;
    db.execute_batch("INSERT INTO fleet_allocations SELECT project_id,issue_number,machine FROM worker_allocation_winners WHERE true
        ON CONFLICT(project_id,issue_number) DO UPDATE SET node=excluded.node WHERE fleet_allocations.node<>excluded.node;
        DELETE FROM fleet_allocation_deadlines WHERE (project_id,issue_number) IN (SELECT project_id,issue_number FROM worker_allocation_winners);")?;
    Ok(())
}

/// Renew only while the local worker process still owns the attempt. Restarts
/// cannot resurrect a dead process; remote replicas expire its last heartbeat.
pub(crate) fn maintain(db: &Connection) -> Result<()> {
    if !enabled(db)? {
        return Ok(());
    }
    let clock = Clock::read()?;
    let due: bool = db.query_row(
        "SELECT boot<>?1 OR renewed_tick+?3<=?2 FROM worker_allocation_clock WHERE id=1",
        params![clock.boot, clock.tick, ms(SYNC_INTERVAL)],
        |r| r.get(0),
    )?;
    if !due {
        return Ok(());
    }
    let tx = db.unchecked_transaction()?;
    // Several transports can arrive together. Recheck under the write transaction.
    if !tx.query_row(
        "SELECT boot<>?1 OR renewed_tick+?3<=?2 FROM worker_allocation_clock WHERE id=1",
        params![clock.boot, clock.tick, ms(SYNC_INTERVAL)],
        |r| r.get::<_, bool>(0),
    )? {
        tx.commit()?;
        return Ok(());
    }
    let exchange: i64 = tx.query_row(
        "SELECT coalesce(max(exchange_ms),0) FROM distributed_sync_timings",
        [],
        |r| r.get(0),
    )?;
    let timing = Timing::from_exchange(Duration::from_millis(exchange.max(0) as u64));
    tx.execute("UPDATE worker_allocation_clock SET boot=?1,renewed_tick=?2,settle_ms=?3,lease_ms=?4,presence_sequence=presence_sequence+1 WHERE id=1", params![clock.boot,clock.tick,ms(timing.settle),ms(timing.lease)])?;
    let runs = tx.query_collect("SELECT id,owner_pid,owner_start,project_id,issue_number,machine,state FROM worker_runs WHERE machine=(SELECT node FROM fleet_meta WHERE id=1) AND finished_at IS NULL", [], |r| Ok::<_,rusqlite::Error>((r.get::<_,String>(0)?,r.get::<_,u32>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,i64>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?)))?;
    let mut active = Vec::new();
    for (id, pid, start, project, number, machine, state) in runs {
        if crate::agents::process_identity(pid).as_deref() != Some(start.as_str()) {
            tx.execute(
                "UPDATE worker_allocations SET released=1 WHERE id=?1 AND released=0",
                [&id],
            )?;
            continue;
        }
        // Adopt pre-upgrade attempts without restarting them. An existing agent
        // precedes new reservations; simultaneous legacy agents tie by identity.
        tx.execute("INSERT OR IGNORE INTO worker_allocations(id,project_id,issue_number,machine,ticket,launched,settle_ms,lease_ms) VALUES(?1,?2,?3,?4,0,?5,?6,?7)", params![id,project,number,machine,(state!="reserved").then_some(0),ms(timing.settle),ms(timing.lease)])?;
        active.push(id);
    }
    tx.execute("INSERT INTO worker_allocation_presence SELECT node,presence_sequence,?1,?2,?3,lease_ms FROM fleet_meta CROSS JOIN worker_allocation_clock WHERE fleet_meta.id=1 AND worker_allocation_clock.id=1
        ON CONFLICT(machine) DO UPDATE SET sequence=excluded.sequence,attempts=excluded.attempts,boot=excluded.boot,seen_tick=excluded.seen_tick,remaining_ms=excluded.remaining_ms",params![serde_json::to_string(&active)?,clock.boot,clock.tick])?;
    reconcile(&tx, &clock)?;
    tx.commit()?;
    Ok(())
}

pub(crate) fn presence(db: &Connection) -> Result<Vec<hey_boss_sync::session::Presence>> {
    let clock = Clock::read()?;
    let rows=db.query_collect("SELECT machine,sequence,attempts,remaining_ms-(?2-seen_tick) FROM worker_allocation_presence WHERE boot=?1 AND seen_tick+remaining_ms>?2",params![clock.boot,clock.tick],|r|Ok::<_,rusqlite::Error>((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?)))?;
    rows.into_iter()
        .map(|(machine, sequence, attempts, remaining_ms)| {
            Ok(hey_boss_sync::session::Presence {
                machine,
                sequence,
                attempts: serde_json::from_str(&attempts)?,
                remaining_ms,
            })
        })
        .collect()
}

pub(crate) fn receive_presence(
    db: &Connection,
    pulses: &[hey_boss_sync::session::Presence],
    elapsed: Duration,
) -> Result<()> {
    let clock = Clock::read()?;
    let tx = db.unchecked_transaction()?;
    for pulse in pulses {
        if pulse.machine.is_empty() || pulse.sequence < 0 || pulse.remaining_ms < 0 {
            return Err(Error::invalid("Invalid allocation presence"));
        }
        // Deduct the entire exchange time as a conservative network allowance.
        // Never accept echoed evidence for this machine or refresh a replay.
        tx.execute("INSERT INTO worker_allocation_presence SELECT ?1,?2,?3,?4,?5,?6 WHERE ?1<>(SELECT node FROM fleet_meta WHERE id=1)
            ON CONFLICT(machine) DO UPDATE SET sequence=excluded.sequence,attempts=excluded.attempts,boot=excluded.boot,seen_tick=excluded.seen_tick,remaining_ms=excluded.remaining_ms
            WHERE excluded.sequence>worker_allocation_presence.sequence",params![pulse.machine,pulse.sequence,serde_json::to_string(&pulse.attempts)?,clock.boot,clock.tick,pulse.remaining_ms.saturating_sub(ms(elapsed)).max(0)])?;
    }
    reconcile(&tx, &clock)?;
    tx.commit()?;
    Ok(())
}

pub(crate) fn owns(db: &Connection, id: &str, project: &str, number: i64) -> Result<bool> {
    if !enabled(db)? {
        return Ok(true);
    }
    Ok(db.query_row("SELECT (NOT EXISTS(SELECT 1 FROM worker_allocations WHERE id=?1) OR EXISTS(SELECT 1 FROM worker_allocation_winners WHERE project_id=?2 AND issue_number=?3 AND id=?1)) AND NOT EXISTS(SELECT 1 FROM issues i JOIN worker_allocations a ON a.id=?1 WHERE i.project_id=?2 AND i.number=?3 AND i.assignment_target LIKE 'machine:%' AND i.assignment_target<>'machine:'||a.machine)", params![id,project,number], |r| r.get(0))?)
}

/// Presence travels ahead of the durable journal. Until every announced attempt
/// has its reservation, the replica cannot know which tasks are already owned.
/// Expiring a heartbeat cannot prove those missing facts do not matter; only a
/// newer presence list or delivery of the reservation clears this hold.
pub(crate) fn pending_reservations(db: &Connection) -> Result<i64> {
    Ok(db.query_row("SELECT count(*) FROM worker_allocation_presence p,json_each(p.attempts) attempt
        WHERE NOT EXISTS(SELECT 1 FROM worker_allocations a WHERE a.id=attempt.value AND a.machine=p.machine)",[],|r|r.get(0))?)
}

pub(crate) fn conflicting_attempts(db: &Connection) -> Result<i64> {
    Ok(db.query_row("SELECT count(*) FROM worker_runs r JOIN worker_allocation_winners w ON w.project_id=r.project_id AND w.issue_number=r.issue_number
        WHERE r.finished_at IS NULL AND r.state<>'reserved' AND r.machine=(SELECT node FROM fleet_meta WHERE id=1) AND r.id<>w.id",[],|r|r.get(0))?)
}

/// Atomically recheck the settled winner and publish launch intent before any
/// provider process can execute work. Concurrent launches converge by the same
/// Lamport ticket and stable identity order on every replica.
pub(crate) fn launch(db: &Connection, id: &str) -> Result<Option<Duration>> {
    if !enabled(db)? {
        return Ok(None);
    }
    let clock = Clock::read()?;
    let tx = db.unchecked_transaction()?;
    reconcile(&tx, &clock)?;
    let row: Option<(String,i64,i64,Option<i64>)>=tx.query_row("SELECT a.project_id,a.issue_number,o.ready_tick,a.launched FROM worker_allocations a JOIN worker_allocation_observations o ON o.id=a.id WHERE a.id=?1 AND a.released=0", [id], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let Some((project, number, ready, launched)) = row else {
        tx.commit()?;
        return Err(Error::new(
            "cancelled",
            "Machine reservation expired or was released",
        ));
    };
    if !owns(&tx, id, &project, number)? {
        tx.commit()?;
        return Err(Error::new(
            "cancelled",
            "Another machine owns this task; saved work is retained",
        ));
    }
    if pending_reservations(&tx)? > 0 {
        tx.commit()?;
        return Ok(Some(SYNC_INTERVAL));
    }
    if launched.is_none() && ready > clock.tick {
        tx.commit()?;
        return Ok(Some(Duration::from_millis((ready - clock.tick) as u64)));
    }
    if launched.is_none() {
        tx.execute(
            "UPDATE worker_allocation_clock SET clock=clock+1 WHERE id=1",
            [],
        )?;
        tx.execute("UPDATE worker_allocations SET launched=(SELECT clock FROM worker_allocation_clock WHERE id=1) WHERE id=?1", [id])?;
        reconcile(&tx, &clock)?;
    }
    tx.commit()?;
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (crate::admin::Temporary, Connection) {
        let root = crate::admin::Temporary::new().unwrap();
        let db = super::super::Store::open(&root.0.join("issues.db"))
            .unwrap()
            .into_database();
        db.execute("UPDATE fleet_meta SET role='peer',node='local'", [])
            .unwrap();
        (root, db)
    }
    fn clock(tick: i64) -> Clock {
        Clock {
            boot: "boot-a".into(),
            tick,
        }
    }
    fn reserve(db: &Connection, id: &str, machine: &str, ticket: i64, launched: Option<i64>) {
        let timing = Timing::from_exchange(Duration::ZERO);
        db.execute("INSERT INTO worker_allocations(id,project_id,issue_number,machine,ticket,launched,settle_ms,lease_ms) VALUES(?1,'p',1,?2,?3,?4,?5,?6)",params![id,machine,ticket,launched,ms(timing.settle),ms(timing.lease)]).unwrap();
    }
    fn winner(db: &Connection) -> Option<String> {
        db.query_row("SELECT id FROM worker_allocation_winners", [], |r| r.get(0))
            .optional()
            .unwrap()
    }
    fn pulse(
        db: &Connection,
        machine: &str,
        sequence: i64,
        attempts: &str,
        at: i64,
        remaining: i64,
    ) {
        db.execute("INSERT INTO worker_allocation_presence VALUES(?1,?2,?3,'boot-a',?4,?5) ON CONFLICT(machine) DO UPDATE SET sequence=excluded.sequence,attempts=excluded.attempts,seen_tick=excluded.seen_tick,remaining_ms=excluded.remaining_ms",params![machine,sequence,attempts,at,remaining]).unwrap();
    }
    #[test]
    fn concurrent_allocations_and_launches_converge_independent_of_arrival_order() {
        for reverse in [false, true] {
            let (_root, db) = fixture();
            let mut rows = vec![("a", "machine-a", 1, None), ("b", "machine-b", 1, None)];
            if reverse {
                rows.reverse();
            }
            for (id, machine, ticket, launched) in rows {
                reserve(&db, id, machine, ticket, launched);
                reconcile(&db, &clock(0)).unwrap();
            }
            assert_eq!(winner(&db).as_deref(), Some("a"));
            db.execute("UPDATE worker_allocations SET launched=20 WHERE id='b'", [])
                .unwrap();
            reconcile(&db, &clock(1)).unwrap();
            assert_eq!(
                winner(&db).as_deref(),
                Some("b"),
                "Launched agent precedes pending reservations"
            );
            db.execute("UPDATE worker_allocations SET launched=20 WHERE id='a'", [])
                .unwrap();
            reconcile(&db, &clock(2)).unwrap();
            assert_eq!(
                winner(&db).as_deref(),
                Some("a"),
                "Concurrent launches use a stable identity tie break"
            );
            db.execute("UPDATE worker_allocations SET released=1 WHERE id='b'", [])
                .unwrap();
            reconcile(&db, &clock(3)).unwrap();
            assert_eq!(
                winner(&db).as_deref(),
                Some("a"),
                "A losing run cannot release the winner"
            );
        }
    }
    #[test]
    fn idle_reconciliation_does_not_write_or_extend_deadlines() {
        let (_root, db) = fixture();
        reserve(&db, "a", "machine-a", 1, None);
        reconcile(&db, &clock(0)).unwrap();
        let before: i64 = db
            .query_row("SELECT total_changes()", [], |r| r.get(0))
            .unwrap();
        reconcile(&db, &clock(1)).unwrap();
        let after: i64 = db
            .query_row("SELECT total_changes()", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            before, after,
            "Unchanged projections must not rewrite allocations"
        );
        reconcile(&db, &clock(ms(Timing::from_exchange(Duration::ZERO).lease))).unwrap();
        assert_eq!(winner(&db), None);
        reconcile(&db, &clock(100_000)).unwrap();
        assert_eq!(winner(&db), None);
    }
    #[test]
    fn renewal_requires_new_evidence_and_a_reboot_expires_old_observations() {
        let (_root, db) = fixture();
        reserve(&db, "a", "machine-a", 1, None);
        reconcile(&db, &clock(0)).unwrap();
        pulse(&db, "machine-a", 1, "[\"a\"]", 100, 10);
        reconcile(&db, &clock(100)).unwrap();
        assert_eq!(winner(&db).as_deref(), Some("a"));
        reconcile(&db, &clock(110)).unwrap();
        assert_eq!(winner(&db), None);
        // Even a launch update cannot renew the liveness lease.
        db.execute("UPDATE worker_allocations SET launched=2", [])
            .unwrap();
        reconcile(&db, &clock(111)).unwrap();
        assert_eq!(winner(&db), None);
        pulse(&db, "machine-a", 2, "[\"a\"]", 112, 10);
        reconcile(&db, &clock(112)).unwrap();
        assert_eq!(winner(&db).as_deref(), Some("a"));
        reconcile(
            &db,
            &Clock {
                boot: "boot-b".into(),
                tick: 0,
            },
        )
        .unwrap();
        assert_eq!(winner(&db), None);
    }
    #[test]
    fn presence_before_reservation_does_not_invalidate_it_and_absence_after_does() {
        let (_root, db) = fixture();
        reserve(&db, "a", "machine-a", 1, None);
        db.execute("UPDATE worker_allocations SET presence_after=2", [])
            .unwrap();
        pulse(&db, "machine-a", 1, "[]", 0, 100);
        reconcile(&db, &clock(0)).unwrap();
        assert_eq!(winner(&db).as_deref(), Some("a"));
        pulse(&db, "machine-a", 2, "[]", 1, 100);
        reconcile(&db, &clock(1)).unwrap();
        assert_eq!(winner(&db), None);
    }
    #[test]
    fn replay_and_relay_cannot_restart_presence_lifetime() {
        let (_root, db) = fixture();
        reserve(&db, "a", "machine-a", 1, None);
        let pulse = hey_boss_sync::session::Presence {
            machine: "machine-a".into(),
            sequence: 1,
            attempts: vec!["a".into()],
            remaining_ms: ms(SYNC_INTERVAL),
        };
        receive_presence(&db, &[pulse.clone()], SYNC_INTERVAL / 2).unwrap();
        let first = presence(&db).unwrap().remove(0);
        assert!(first.remaining_ms <= ms(SYNC_INTERVAL / 2));
        receive_presence(&db, &[pulse.clone()], Duration::ZERO).unwrap();
        assert!(presence(&db).unwrap()[0].remaining_ms <= first.remaining_ms);
        let (_relay, relay) = fixture();
        receive_presence(&relay, &[first], SYNC_INTERVAL).unwrap();
        assert!(presence(&relay).unwrap().is_empty());
        receive_presence(&relay, &[pulse], Duration::ZERO).unwrap();
        assert!(
            presence(&relay).unwrap().is_empty(),
            "A replay cannot revive an expired pulse"
        );
    }
    #[test]
    fn crashed_workers_release_reservations_and_restart_never_renews_them() {
        let (_root, db) = fixture();
        db.execute_batch("INSERT INTO projects(id,name,next_number) VALUES('p','Project',2);
            INSERT INTO agents VALUES('agent','{}',0);
            INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels) VALUES('p',1,'Task','','open','agent',1,1,1,'[]');
            INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at)
            VALUES('a','p',1,'{}','agent','reserved',1,'wrong-process-identity','local',0,0)").unwrap();
        reconcile(&db, &Clock::read().unwrap()).unwrap();
        assert_eq!(winner(&db).as_deref(), Some("a"));
        maintain(&db).unwrap();
        assert_eq!(winner(&db), None);
        assert!(presence(&db).unwrap()[0].attempts.is_empty());
        db.execute(
            "UPDATE worker_runs SET owner_pid=?1,owner_start=?2",
            params![
                std::process::id(),
                crate::agents::process_identity(std::process::id()).unwrap()
            ],
        )
        .unwrap();
        db.execute(
            "UPDATE worker_allocation_clock SET boot='previous-boot'",
            [],
        )
        .unwrap();
        maintain(&db).unwrap();
        assert_eq!(
            winner(&db),
            None,
            "Released attempts require a fresh reservation, even if a PID is reused"
        );
    }
    #[test]
    fn launch_waits_for_reservations_announced_ahead_of_the_replication_journal() {
        let (_root, db) = fixture();
        reserve(&db, "a", "local", 2, None);
        let now = Clock::read().unwrap();
        reconcile(&db, &now).unwrap();
        db.execute("UPDATE worker_allocation_observations SET ready_tick=0", [])
            .unwrap();
        // Presence bypasses historical journal pages. It proves another machine
        // has an attempt, but we do not yet know which issue it owns.
        db.execute(
            "INSERT INTO worker_allocation_presence VALUES('other',1,'[\"b\"]',?1,?2,60000)",
            params![now.boot, now.tick],
        )
        .unwrap();
        assert!(
            launch(&db, "a").unwrap().is_some(),
            "A timer cannot prove ownership while announced reservations are missing"
        );
        let launched: Option<i64> = db
            .query_row(
                "SELECT launched FROM worker_allocations WHERE id='a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(launched, None);
        db.execute(
            "UPDATE worker_allocation_presence SET remaining_ms=0,seen_tick=0",
            [],
        )
        .unwrap();
        assert!(
            launch(&db, "a").unwrap().is_some(),
            "An expired heartbeat cannot make missing ownership facts safe to ignore"
        );
        db.execute(
            "UPDATE worker_allocation_presence SET remaining_ms=60000,seen_tick=?1",
            [Clock::read().unwrap().tick],
        )
        .unwrap();
        reserve(&db, "b", "other", 1, None);
        assert_eq!(launch(&db, "a").unwrap_err().code, "cancelled");
    }

    #[test]
    fn a_new_presence_that_retires_an_unknown_attempt_unblocks_pickup() {
        let (_root, db) = fixture();
        reserve(&db, "a", "local", 1, None);
        let clock = Clock::read().unwrap();
        reconcile(&db, &clock).unwrap();
        db.execute("UPDATE worker_allocation_observations SET ready_tick=0", [])
            .unwrap();
        db.execute(
            "INSERT INTO worker_allocation_presence VALUES('other',1,'[\"missing\"]',?1,?2,60000)",
            params![clock.boot, clock.tick],
        )
        .unwrap();
        assert!(launch(&db, "a").unwrap().is_some());
        receive_presence(
            &db,
            &[hey_boss_sync::session::Presence {
                machine: "other".into(),
                sequence: 2,
                attempts: vec![],
                remaining_ms: 60000,
            }],
            Duration::ZERO,
        )
        .unwrap();
        assert!(launch(&db, "a").unwrap().is_none());
    }

    #[test]
    fn settling_uses_local_monotonic_time_and_rechecks_ownership() {
        let (_root, db) = fixture();
        reserve(&db, "a", "local", 1, None);
        let now = Clock::read().unwrap();
        reconcile(&db, &now).unwrap();
        assert!(launch(&db, "a").unwrap().is_some());
        db.execute(
            "UPDATE worker_allocation_observations SET ready_tick=?1",
            [now.tick - 1],
        )
        .unwrap();
        assert!(launch(&db, "a").unwrap().is_none());
        reserve(&db, "b", "other", 0, Some(0));
        assert_eq!(launch(&db, "a").unwrap_err().code, "cancelled");
    }
}
