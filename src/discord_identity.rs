//! Discord identity read from the local IPC `READY` handshake.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, mpsc};
use std::time::Duration;

use discord_rich_presence::{DiscordIpc, DiscordIpcClient};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Identity {
    pub id: String,
    pub username: String,
    pub global_name: Option<String>,
    pub avatar: Option<String>,
    pub banner: Option<String>,
    #[serde(default)]
    pub discriminator: String,
}

static QUERY_RUNNING: AtomicBool = AtomicBool::new(false);
static LAST_IDENTITY: OnceLock<Mutex<Option<Identity>>> = OnceLock::new();

pub fn current_user() -> Option<Identity> {
    if QUERY_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return LAST_IDENTITY
            .get_or_init(|| Mutex::new(None))
            .lock()
            .ok()?
            .clone();
    }
    let (send, receive) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let identity = read_ready_identity();
        if let Some(identity) = &identity
            && let Ok(mut last) = LAST_IDENTITY.get_or_init(|| Mutex::new(None)).lock()
        {
            *last = Some(identity.clone());
        }
        QUERY_RUNNING.store(false, Ordering::Release);
        let _ = send.send(identity);
    });
    receive.recv_timeout(Duration::from_secs(2)).ok().flatten()
}

fn read_ready_identity() -> Option<Identity> {
    let id = crate::opencode::Config::default().client_id;
    let mut client = DiscordIpcClient::new(&id);
    client.connect_ipc().ok()?;
    client
        .send(serde_json::json!({"v": 1, "client_id": id}), 0)
        .ok()?;
    let (_, response) = client.recv().ok()?;
    let identity = identity_from_ready(&response);
    let _ = client.close();
    identity
}

fn identity_from_ready(response: &serde_json::Value) -> Option<Identity> {
    if response.get("evt").and_then(serde_json::Value::as_str) != Some("READY") {
        return None;
    }
    let identity: Identity =
        serde_json::from_value(response.pointer("/data/user")?.clone()).ok()?;
    (!identity.username.is_empty()
        && (17..=22).contains(&identity.id.len())
        && identity.id.bytes().all(|byte| byte.is_ascii_digit()))
    .then_some(identity)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ready_identity_is_authoritative_and_does_not_copy_credentials() {
        let ready = serde_json::json!({"evt":"READY","data":{"user":{"id":"123456789012345678","username":"current-user","global_name":"Current user","avatar":"abc","token":"never-copy"}}});
        let identity = identity_from_ready(&ready).unwrap();
        assert_eq!(identity.username, "current-user");
        assert_eq!(identity.avatar.as_deref(), Some("abc"));
        assert!(
            !serde_json::to_string(&identity)
                .unwrap()
                .contains("never-copy")
        );
        assert!(
            identity_from_ready(&serde_json::json!({"evt":"ERROR","data":ready["data"]})).is_none()
        );
    }
}
