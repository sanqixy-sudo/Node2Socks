//! Stable Proxy Slot persistence and conservative local port allocation.

use node2socks_domain::{AppError, AppResult, ErrorCode, ProxySlot, SlotBinding, SlotBindingState};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    net::{Ipv4Addr, SocketAddrV4, TcpListener},
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortConflict {
    pub port: u16,
    pub pid: Option<u32>,
    pub process_name: Option<String>,
}

pub trait PortProbe: Send + Sync {
    fn is_available(&self, port: u16) -> bool;
    fn owner(&self, port: u16) -> Option<(u32, String)>;
}

#[derive(Debug, Default)]
pub struct SystemPortProbe;

impl PortProbe for SystemPortProbe {
    fn is_available(&self, port: u16) -> bool {
        TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)).is_ok()
    }

    fn owner(&self, port: u16) -> Option<(u32, String)> {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            let output = Command::new("netstat")
                .args(["-ano", "-p", "tcp"])
                .creation_flags(CREATE_NO_WINDOW)
                .output()
                .ok()?;
            let text = String::from_utf8_lossy(&output.stdout);
            let suffix = format!(":{port}");
            let pid = text.lines().find_map(|line| {
                let fields: Vec<_> = line.split_whitespace().collect();
                (fields.len() >= 5
                    && fields[1].ends_with(&suffix)
                    && fields[3].eq_ignore_ascii_case("LISTENING"))
                .then(|| fields[4].parse::<u32>().ok())
                .flatten()
            })?;
            let filter = format!("PID eq {pid}");
            let names = Command::new("tasklist")
                .args(["/FI", &filter, "/FO", "CSV", "/NH"])
                .creation_flags(CREATE_NO_WINDOW)
                .output()
                .ok()?;
            let row = String::from_utf8_lossy(&names.stdout);
            let name = row
                .lines()
                .next()?
                .split(',')
                .next()?
                .trim_matches('"')
                .to_owned();
            Some((pid, name))
        }
        #[cfg(not(windows))]
        {
            let _ = port;
            None
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PortRange {
    pub start: u16,
    pub end: u16,
}

impl PortRange {
    pub fn validate(self) -> AppResult<Self> {
        if self.start == 0 || self.start > self.end {
            return Err(AppError::new(
                ErrorCode::InvalidConfiguration,
                "port range must be non-zero and ascending",
            ));
        }
        Ok(self)
    }
}

pub struct PortAllocator<P: PortProbe> {
    range: PortRange,
    cooldown: Duration,
    probe: P,
}

impl<P: PortProbe> PortAllocator<P> {
    pub fn new(range: PortRange, cooldown: Duration, probe: P) -> AppResult<Self> {
        Ok(Self {
            range: range.validate()?,
            cooldown,
            probe,
        })
    }

    pub fn allocate(
        &self,
        used: &HashSet<u16>,
        released_at: &HashMap<u16, SystemTime>,
        now: SystemTime,
    ) -> AppResult<u16> {
        for port in self.range.start..=self.range.end {
            if used.contains(&port) || !self.probe.is_available(port) {
                continue;
            }
            if released_at
                .get(&port)
                .and_then(|released| now.duration_since(*released).ok())
                .is_some_and(|elapsed| elapsed < self.cooldown)
            {
                continue;
            }
            return Ok(port);
        }
        Err(AppError::new(
            ErrorCode::InvalidConfiguration,
            "configured Proxy Slot port range is exhausted",
        ))
    }

    /// A restored/synced Slot owns its saved port. Conflict is surfaced, never renumbered.
    pub fn validate_stable_port(&self, port: u16) -> Result<(), PortConflict> {
        if self.probe.is_available(port) {
            return Ok(());
        }
        let owner = self.probe.owner(port);
        Err(PortConflict {
            port,
            pid: owner.as_ref().map(|value| value.0),
            process_name: owner.map(|value| value.1),
        })
    }
}

pub trait SlotRepository: Send + Sync {
    fn list(&self) -> AppResult<Vec<(ProxySlot, SlotBinding)>>;
    fn create(&self, slot: &ProxySlot, binding: &SlotBinding) -> AppResult<()>;
    fn bind(&self, slot_id: Uuid, node_id: Option<Uuid>, state: SlotBindingState) -> AppResult<()>;
    fn delete_with_cooldown(&self, slot_id: Uuid, cooldown: Duration) -> AppResult<()>;
    fn used_ports(&self) -> AppResult<HashSet<u16>>;
    fn cooldowns(&self) -> AppResult<HashMap<u16, SystemTime>>;
}

#[derive(Clone)]
pub struct SqliteSlotRepository {
    connection: Arc<Mutex<Connection>>,
}

impl SqliteSlotRepository {
    pub fn new(connection: Connection) -> Self {
        Self {
            connection: Arc::new(Mutex::new(connection)),
        }
    }
}

impl SlotRepository for SqliteSlotRepository {
    fn list(&self) -> AppResult<Vec<(ProxySlot, SlotBinding)>> {
        let connection = self.connection.lock().map_err(lock_error)?;
        let mut statement = connection
            .prepare(
                "SELECT s.id,s.name,s.local_port,s.listen_host,s.enabled,s.sync_version,\
                        b.node_id,b.state,b.sync_version \
                 FROM proxy_slots s JOIN slot_bindings b ON b.slot_id=s.id \
                 ORDER BY s.local_port",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map([], |row| {
                let slot_id = parse_uuid(row.get::<_, String>(0)?)?;
                let node_id = row
                    .get::<_, Option<String>>(6)?
                    .map(parse_uuid)
                    .transpose()?;
                Ok((
                    ProxySlot {
                        id: slot_id,
                        name: row.get(1)?,
                        local_port: row.get(2)?,
                        listen_host: row.get(3)?,
                        enabled: row.get::<_, i64>(4)? != 0,
                        revision: row.get(5)?,
                    },
                    SlotBinding {
                        slot_id,
                        node_id,
                        state: parse_binding_state(&row.get::<_, String>(7)?)?,
                        revision: row.get(8)?,
                    },
                ))
            })
            .map_err(database_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(database_error)
    }

    fn create(&self, slot: &ProxySlot, binding: &SlotBinding) -> AppResult<()> {
        let mut connection = self.connection.lock().map_err(lock_error)?;
        let transaction = connection.transaction().map_err(database_error)?;
        let now = unix_seconds(SystemTime::now())?.to_string();
        transaction
            .execute(
                "INSERT INTO proxy_slots \
                 (id,name,local_port,listen_host,enabled,created_at,updated_at,sync_version) \
                 VALUES (?1,?2,?3,?4,?5,?6,?6,?7)",
                params![
                    slot.id.to_string(),
                    slot.name,
                    slot.local_port,
                    slot.listen_host,
                    slot.enabled,
                    now,
                    slot.revision
                ],
            )
            .map_err(database_error)?;
        transaction
            .execute(
                "INSERT INTO slot_bindings \
                 (slot_id,node_id,state,updated_at,sync_version) VALUES (?1,?2,?3,?4,?5)",
                params![
                    binding.slot_id.to_string(),
                    binding.node_id.map(|id| id.to_string()),
                    binding_state_name(binding.state),
                    now,
                    binding.revision
                ],
            )
            .map_err(database_error)?;
        transaction.commit().map_err(database_error)
    }

    fn bind(&self, slot_id: Uuid, node_id: Option<Uuid>, state: SlotBindingState) -> AppResult<()> {
        let connection = self.connection.lock().map_err(lock_error)?;
        let changed = connection
            .execute(
                "UPDATE slot_bindings SET node_id=?2,state=?3,updated_at=?4,\
                 sync_version=sync_version+1 WHERE slot_id=?1",
                params![
                    slot_id.to_string(),
                    node_id.map(|id| id.to_string()),
                    binding_state_name(state),
                    unix_seconds(SystemTime::now())?.to_string()
                ],
            )
            .map_err(database_error)?;
        if changed == 0 {
            return Err(AppError::new(
                ErrorCode::InvalidConfiguration,
                "Proxy Slot does not exist",
            ));
        }
        Ok(())
    }

    fn delete_with_cooldown(&self, slot_id: Uuid, cooldown: Duration) -> AppResult<()> {
        let mut connection = self.connection.lock().map_err(lock_error)?;
        let transaction = connection.transaction().map_err(database_error)?;
        let port = transaction
            .query_row(
                "SELECT local_port FROM proxy_slots WHERE id=?1",
                [slot_id.to_string()],
                |row| row.get::<_, u16>(0),
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| {
                AppError::new(ErrorCode::InvalidConfiguration, "Proxy Slot does not exist")
            })?;
        let released = SystemTime::now();
        let reusable = released + cooldown;
        transaction
            .execute("DELETE FROM proxy_slots WHERE id=?1", [slot_id.to_string()])
            .map_err(database_error)?;
        transaction
            .execute(
                "INSERT INTO port_cooldowns(local_port,released_at,reusable_after) \
                 VALUES (?1,?2,?3) ON CONFLICT(local_port) DO UPDATE SET \
                 released_at=excluded.released_at,reusable_after=excluded.reusable_after",
                params![
                    port,
                    unix_seconds(released)?.to_string(),
                    unix_seconds(reusable)?.to_string()
                ],
            )
            .map_err(database_error)?;
        transaction.commit().map_err(database_error)
    }

    fn used_ports(&self) -> AppResult<HashSet<u16>> {
        let connection = self.connection.lock().map_err(lock_error)?;
        let mut statement = connection
            .prepare("SELECT local_port FROM proxy_slots")
            .map_err(database_error)?;
        let ports = statement
            .query_map([], |row| row.get(0))
            .map_err(database_error)?;
        ports
            .collect::<Result<HashSet<_>, _>>()
            .map_err(database_error)
    }

    fn cooldowns(&self) -> AppResult<HashMap<u16, SystemTime>> {
        let connection = self.connection.lock().map_err(lock_error)?;
        let mut statement = connection
            .prepare("SELECT local_port,released_at FROM port_cooldowns")
            .map_err(database_error)?;
        let rows = statement
            .query_map([], |row| {
                let port = row.get::<_, u16>(0)?;
                let seconds = row.get::<_, String>(1)?.parse::<u64>().map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
                Ok((port, UNIX_EPOCH + Duration::from_secs(seconds)))
            })
            .map_err(database_error)?;
        rows.collect::<Result<HashMap<_, _>, _>>()
            .map_err(database_error)
    }
}

fn unix_seconds(time: SystemTime) -> AppResult<u64> {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| AppError::new(ErrorCode::InvalidConfiguration, error.to_string()))
}

fn binding_state_name(state: SlotBindingState) -> &'static str {
    match state {
        SlotBindingState::Active => "active",
        SlotBindingState::Orphaned => "orphaned",
        SlotBindingState::Unbound => "unbound",
        SlotBindingState::Blocked => "blocked",
        SlotBindingState::Error => "error",
    }
}

fn parse_binding_state(value: &str) -> rusqlite::Result<SlotBindingState> {
    match value {
        "active" => Ok(SlotBindingState::Active),
        "orphaned" => Ok(SlotBindingState::Orphaned),
        "unbound" => Ok(SlotBindingState::Unbound),
        "blocked" => Ok(SlotBindingState::Blocked),
        "error" => Ok(SlotBindingState::Error),
        other => Err(rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            format!("invalid Slot binding state: {other}").into(),
        )),
    }
}

fn parse_uuid(value: String) -> rusqlite::Result<Uuid> {
    Uuid::parse_str(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })
}

fn database_error(error: rusqlite::Error) -> AppError {
    AppError::new(ErrorCode::DatabaseError, error.to_string())
}

fn lock_error<T>(error: std::sync::PoisonError<T>) -> AppError {
    AppError::new(ErrorCode::DatabaseError, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use node2socks_storage::open_in_memory_and_migrate;

    #[derive(Default)]
    struct FakeProbe {
        occupied: HashMap<u16, (u32, String)>,
    }

    impl PortProbe for FakeProbe {
        fn is_available(&self, port: u16) -> bool {
            !self.occupied.contains_key(&port)
        }

        fn owner(&self, port: u16) -> Option<(u32, String)> {
            self.occupied.get(&port).cloned()
        }
    }
    #[test]
    fn repository_survives_reopen_with_same_port_and_binding() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("slots.db");
        let node = Uuid::new_v4();
        {
            let connection = node2socks_storage::open_and_migrate(&path).unwrap();
            connection.execute("INSERT INTO subscriptions(id,name,url_cipher,created_at,updated_at) VALUES('00000000-0000-0000-0000-000000000001','s',x'00','0','0')",[]).unwrap();
            connection.execute("INSERT INTO nodes(id,subscription_id,stable_key,internal_name,upstream_name,provider_name,created_at,updated_at) VALUES(?1,'00000000-0000-0000-0000-000000000001','k','node','node','p','0','0')",[node.to_string()]).unwrap();
            let repository = SqliteSlotRepository::new(connection);
            let slot = ProxySlot::new("stable", 21_055).unwrap();
            repository
                .create(
                    &slot,
                    &SlotBinding {
                        slot_id: slot.id,
                        node_id: Some(node),
                        state: SlotBindingState::Active,
                        revision: 3,
                    },
                )
                .unwrap();
        }
        let repository =
            SqliteSlotRepository::new(node2socks_storage::open_and_migrate(&path).unwrap());
        let (slot, binding) = repository.list().unwrap().remove(0);
        assert_eq!(slot.local_port, 21_055);
        assert_eq!(binding.node_id, Some(node));
        assert_eq!(binding.state, SlotBindingState::Active);
    }

    #[test]
    fn one_hundred_slots_persist_with_distinct_stable_ports() {
        let repository = SqliteSlotRepository::new(open_in_memory_and_migrate().unwrap());
        for offset in 0..100_u16 {
            let slot = ProxySlot::new(format!("Slot {}", offset + 1), 21_000 + offset).unwrap();
            repository
                .create(
                    &slot,
                    &SlotBinding {
                        slot_id: slot.id,
                        node_id: None,
                        state: SlotBindingState::Unbound,
                        revision: 0,
                    },
                )
                .unwrap();
        }
        let items = repository.list().unwrap();
        assert_eq!(items.len(), 100);
        assert_eq!(items.first().unwrap().0.local_port, 21_000);
        assert_eq!(items.last().unwrap().0.local_port, 21_099);
        assert_eq!(
            items
                .iter()
                .map(|item| item.0.id)
                .collect::<HashSet<_>>()
                .len(),
            100
        );
    }

    #[test]
    fn allocator_skips_used_conflicted_and_cooling_ports() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        let allocator = PortAllocator::new(
            PortRange {
                start: 21_000,
                end: 21_004,
            },
            Duration::from_secs(100),
            FakeProbe {
                occupied: HashMap::from([(21_001, (77, "other.exe".into()))]),
            },
        )
        .unwrap();
        let used = HashSet::from([21_000]);
        let cooldowns = HashMap::from([(21_002, now - Duration::from_secs(50))]);
        assert_eq!(allocator.allocate(&used, &cooldowns, now).unwrap(), 21_003);
    }

    #[test]
    fn restored_port_conflict_is_reported_without_renumbering() {
        let allocator = PortAllocator::new(
            PortRange {
                start: 21_000,
                end: 21_999,
            },
            Duration::ZERO,
            FakeProbe {
                occupied: HashMap::from([(21_001, (1234, "chrome.exe".into()))]),
            },
        )
        .unwrap();
        assert_eq!(
            allocator.validate_stable_port(21_001),
            Err(PortConflict {
                port: 21_001,
                pid: Some(1234),
                process_name: Some("chrome.exe".into())
            })
        );
    }

    #[test]
    fn exhausted_range_returns_structured_error() {
        let allocator = PortAllocator::new(
            PortRange {
                start: 21_000,
                end: 21_000,
            },
            Duration::ZERO,
            FakeProbe::default(),
        )
        .unwrap();
        let error = allocator
            .allocate(&HashSet::from([21_000]), &HashMap::new(), SystemTime::now())
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidConfiguration);
    }

    #[test]
    fn repository_persists_binding_and_port_cooldown() {
        let repository = SqliteSlotRepository::new(open_in_memory_and_migrate().unwrap());
        let slot = ProxySlot::new("A", 21_001).unwrap();
        let binding = SlotBinding {
            slot_id: slot.id,
            node_id: None,
            state: SlotBindingState::Unbound,
            revision: 0,
        };
        repository.create(&slot, &binding).unwrap();
        assert_eq!(repository.list().unwrap()[0].0.local_port, 21_001);
        repository
            .delete_with_cooldown(slot.id, Duration::from_secs(600))
            .unwrap();
        assert!(repository.list().unwrap().is_empty());
        assert!(repository.cooldowns().unwrap().contains_key(&21_001));
    }
}
