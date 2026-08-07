//! Native Windows Service host for the Claude Code bridge.
//!
//! This module only owns process lifecycle integration with the Windows
//! Service Control Manager. HTTP routing and model switching remain in
//! `main.rs`, so console and service modes run exactly the same bridge.

use std::{
    ffi::OsString,
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::sync::watch;
use tracing::{error, info};
use windows_service::{
    define_windows_service,
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult, ServiceStatusHandle},
    service_dispatcher,
};

pub const SERVICE_NAME: &str = "ClaudeCodeBridge";
const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

define_windows_service!(ffi_service_main, service_main);

pub fn run_dispatcher() -> Result<(), String> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .map_err(|err| format!("Cannot connect to Windows Service Control Manager: {err}"))
}

fn service_main(_arguments: Vec<OsString>) {
    if let Err(err) = run_service() {
        // Service mode has no usable console. Logging may itself be the startup
        // failure, so this is deliberately only a last-chance diagnostic.
        eprintln!("Claude Code bridge service failed: {err}");
    }
}

fn run_service() -> Result<(), String> {
    let _log_guard = super::init_logging(true)?;
    info!("Claude Code bridge Windows service is starting");

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let status_slot: Arc<Mutex<Option<ServiceStatusHandle>>> = Arc::new(Mutex::new(None));
    let handler_status_slot = Arc::clone(&status_slot);
    let handler_shutdown_tx = shutdown_tx.clone();

    let event_handler = move |control_event| -> ServiceControlHandlerResult {
        match control_event {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                if let Ok(slot) = handler_status_slot.lock() {
                    if let Some(status_handle) = slot.as_ref() {
                        let _ = status_handle.set_service_status(service_status(
                            ServiceState::StopPending,
                            ServiceControlAccept::empty(),
                            ServiceExitCode::Win32(0),
                            Duration::from_secs(30),
                            1,
                        ));
                    }
                }
                let _ = handler_shutdown_tx.send(true);
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };

    let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)
        .map_err(|err| format!("Cannot register Windows service control handler: {err}"))?;
    if let Ok(mut slot) = status_slot.lock() {
        *slot = Some(status_handle);
    }
    status_handle
        .set_service_status(service_status(
            ServiceState::StartPending,
            ServiceControlAccept::empty(),
            ServiceExitCode::Win32(0),
            Duration::from_secs(30),
            1,
        ))
        .map_err(|err| format!("Cannot report service start-pending status: {err}"))?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("Cannot create service Tokio runtime: {err}"))?;
    let running_status_handle = status_handle;
    let bridge_result = runtime.block_on(super::run_bridge(
        shutdown_tx,
        shutdown_rx,
        false,
        move || {
            running_status_handle
                .set_service_status(service_status(
                    ServiceState::Running,
                    ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
                    ServiceExitCode::Win32(0),
                    Duration::default(),
                    0,
                ))
                .map_err(|err| format!("Cannot report service running status: {err}"))
        },
    ));

    let exit_code = if bridge_result.is_ok() {
        ServiceExitCode::Win32(0)
    } else {
        ServiceExitCode::ServiceSpecific(1)
    };
    if let Err(err) = &bridge_result {
        error!("Claude Code bridge service stopped with an error: {err}");
    } else {
        info!("Claude Code bridge Windows service stopped normally");
    }
    status_handle
        .set_service_status(service_status(
            ServiceState::Stopped,
            ServiceControlAccept::empty(),
            exit_code,
            Duration::default(),
            0,
        ))
        .map_err(|err| format!("Cannot report service stopped status: {err}"))?;

    bridge_result
}

fn service_status(
    current_state: ServiceState,
    controls_accepted: ServiceControlAccept,
    exit_code: ServiceExitCode,
    wait_hint: Duration,
    checkpoint: u32,
) -> ServiceStatus {
    ServiceStatus {
        service_type: SERVICE_TYPE,
        current_state,
        controls_accepted,
        exit_code,
        checkpoint,
        wait_hint,
        process_id: None,
    }
}
