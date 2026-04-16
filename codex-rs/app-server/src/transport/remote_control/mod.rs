mod client_tracker;
mod enroll;
mod protocol;
mod websocket;

use crate::transport::remote_control::websocket::RemoteControlWebsocket;
use crate::transport::remote_control::websocket::RemoteControlWebsocketOptions;
use crate::transport::remote_control::websocket::load_remote_control_auth_with_background_agent_task_auth_mode;

pub use self::protocol::ClientId;
use self::protocol::ServerEvent;
use self::protocol::StreamId;
use self::protocol::normalize_remote_control_url;
use super::CHANNEL_CAPACITY;
use super::TransportEvent;
use super::next_connection_id;
use codex_login::AuthManager;
use codex_login::BackgroundAgentTaskAuthMode;
use codex_state::StateRuntime;
use std::io;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub(super) struct QueuedServerEnvelope {
    pub(super) event: ServerEvent,
    pub(super) client_id: ClientId,
    pub(super) stream_id: StreamId,
    pub(super) write_complete_tx: Option<oneshot::Sender<()>>,
}

#[derive(Clone)]
pub(crate) struct RemoteControlHandle {
    enabled_tx: Arc<watch::Sender<bool>>,
}

impl RemoteControlHandle {
    pub(crate) fn set_enabled(&self, enabled: bool) {
        self.enabled_tx.send_if_modified(|state| {
            let changed = *state != enabled;
            *state = enabled;
            changed
        });
    }
}

pub(crate) struct RemoteControlStartOptions {
    pub(crate) remote_control_url: String,
    pub(crate) state_db: Option<Arc<StateRuntime>>,
    pub(crate) auth_manager: Arc<AuthManager>,
    pub(crate) transport_event_tx: mpsc::Sender<TransportEvent>,
    pub(crate) shutdown_token: CancellationToken,
    pub(crate) app_server_client_name_rx: Option<oneshot::Receiver<String>>,
    pub(crate) initial_enabled: bool,
    pub(crate) background_agent_task_auth_mode: BackgroundAgentTaskAuthMode,
}

#[cfg(test)]
pub(crate) async fn start_remote_control(
    remote_control_url: String,
    state_db: Option<Arc<StateRuntime>>,
    auth_manager: Arc<AuthManager>,
    transport_event_tx: mpsc::Sender<TransportEvent>,
    shutdown_token: CancellationToken,
    app_server_client_name_rx: Option<oneshot::Receiver<String>>,
    initial_enabled: bool,
) -> io::Result<(JoinHandle<()>, RemoteControlHandle)> {
    start_remote_control_with_background_agent_task_auth_mode(RemoteControlStartOptions {
        remote_control_url,
        state_db,
        auth_manager,
        transport_event_tx,
        shutdown_token,
        app_server_client_name_rx,
        initial_enabled,
        background_agent_task_auth_mode: BackgroundAgentTaskAuthMode::Enabled,
    })
    .await
}

pub(crate) async fn start_remote_control_with_background_agent_task_auth_mode(
    options: RemoteControlStartOptions,
) -> io::Result<(JoinHandle<()>, RemoteControlHandle)> {
    let RemoteControlStartOptions {
        remote_control_url,
        state_db,
        auth_manager,
        transport_event_tx,
        shutdown_token,
        app_server_client_name_rx,
        initial_enabled,
        background_agent_task_auth_mode,
    } = options;
    let remote_control_target = if initial_enabled {
        Some(normalize_remote_control_url(&remote_control_url)?)
    } else {
        None
    };
    if initial_enabled {
        validate_remote_control_auth_with_background_agent_task_auth_mode(
            &auth_manager,
            &remote_control_url,
            background_agent_task_auth_mode,
        )
        .await?;
    }

    let (enabled_tx, enabled_rx) = watch::channel(initial_enabled);
    let join_handle = tokio::spawn(async move {
        RemoteControlWebsocket::from_options(RemoteControlWebsocketOptions {
            remote_control_url,
            remote_control_target,
            state_db,
            auth_manager,
            transport_event_tx,
            shutdown_token,
            enabled_rx,
            background_agent_task_auth_mode,
        })
        .run(app_server_client_name_rx)
        .await;
    });

    Ok((
        join_handle,
        RemoteControlHandle {
            enabled_tx: Arc::new(enabled_tx),
        },
    ))
}

pub(crate) async fn validate_remote_control_auth_with_background_agent_task_auth_mode(
    auth_manager: &Arc<AuthManager>,
    remote_control_url: &str,
    background_agent_task_auth_mode: BackgroundAgentTaskAuthMode,
) -> io::Result<()> {
    match load_remote_control_auth_with_background_agent_task_auth_mode(
        auth_manager,
        remote_control_url,
        background_agent_task_auth_mode,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::WouldBlock => Ok(()),
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests;
