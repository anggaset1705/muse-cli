//! Windows implementation: one MTA worker thread owns all UIAutomation
//! objects, mirroring .NET `ComputerWorker.cs`.
//!
//! * The worker thread calls `CoInitializeEx(MTA)`, sets per-monitor-V2
//!   DPI awareness, and creates one `IUIAutomation`.
//! * The public `DesktopComputer` submits jobs through a bounded channel
//!   (capacity 1) with a 2 s queue timeout and a 12 s operation timeout —
//!   the same numbers as .NET's `ComputerWorker` (`_queueTimeout`,
//!   `_operationTimeout`). On an operation timeout the shared session is
//!   disarmed, so a hung provider cannot keep control.
//! * All per-session mutable state lives in [`UiState`] on the worker
//!   thread; revocation (`target` / `generation`) lives in the shared
//!   [`Shared`] struct so it takes effect synchronously from any thread.

use crate::actions::{ScreenAction, ScreenArgs, ScreenOutcome};
use crate::computer::Shared;
use crate::error::{ScreenError, ScreenResult};
use crate::geometry::{DesktopWindow, ScreenshotMap, WindowAutoPicker};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{FreeLibrary, HWND};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::LibraryLoader::LoadLibraryW;
use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation, IUIAutomationElement};
use windows::core::w;

pub(crate) mod native;
pub(crate) mod shot;
pub(crate) mod uia;

const OPERATION_TIMEOUT: Duration = Duration::from_secs(12);
const QUEUE_TIMEOUT: Duration = Duration::from_secs(2);

/// Whether the UIAutomation runtime is present on this Windows host
/// (`.NET`: `DesktopComputer.IsRuntimeAvailable` checks the UIAutomation
/// assembly; here we check the underlying `UIAutomationCore.dll`).
pub fn uia_runtime_available() -> bool {
    unsafe {
        match LoadLibraryW(w!("UIAutomationCore.dll")) {
            Ok(module) => {
                let _ = FreeLibrary(module);
                true
            }
            Err(_) => false,
        }
    }
}

/// Per-session mutable state, owned by the worker thread.
pub(crate) struct UiState {
    pub shared: Arc<Shared>,
    pub automation: IUIAutomation,
    pub elements: HashMap<String, IUIAutomationElement>,
    pub identities: HashMap<String, uia::ElementIdentity>,
    pub inspected_at: Option<Instant>,
    pub captured_at: Option<Instant>,
    pub snapshot: String,
    pub image_map: Option<ScreenshotMap>,
    pub capture_window: HWND,
    pub generation: u64,
}

enum Job {
    Execute {
        action: ScreenAction,
        args: ScreenArgs,
        reply: Sender<ScreenResult<ScreenOutcome>>,
    },
    Authorize {
        target: DesktopWindow,
        reply: Sender<ScreenResult<()>>,
    },
    AutoAuthorize {
        hint: Option<String>,
        reply: Sender<ScreenResult<bool>>,
    },
    ListWindows {
        reply: Sender<ScreenResult<Vec<DesktopWindow>>>,
    },
    Preview {
        reply: Sender<ScreenResult<Option<String>>>,
    },
}

/// The MTA worker thread handle (`.NET`: `ComputerWorker`).
pub struct UiWorker {
    tx: SyncSender<Job>,
    shared: Arc<Shared>,
    init_error: Option<ScreenError>,
}

impl UiWorker {
    pub fn new(shared: Arc<Shared>) -> Self {
        let (tx, rx) = sync_channel::<Job>(1);
        let (init_tx, init_rx) = channel::<ScreenResult<()>>();
        let shared2 = Arc::clone(&shared);
        std::thread::spawn(move || worker_main(shared2, rx, init_tx));
        let init_error = match init_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some(e),
            Err(_) => Some(ScreenError::Uia(
                "UI Automation worker did not start.".to_string(),
            )),
        };
        UiWorker {
            tx,
            shared,
            init_error,
        }
    }

    fn submit<T>(&self, make: impl FnOnce(Sender<ScreenResult<T>>) -> Job) -> ScreenResult<T> {
        if let Some(e) = &self.init_error {
            return Err(e.clone());
        }
        let (tx, rx) = channel();
        // `SyncSender::send_timeout` is unstable; spin on `try_send`
        // with the same 2 s queue timeout instead.
        let mut job = make(tx);
        let queued_since = Instant::now();
        loop {
            match self.tx.try_send(job) {
                Ok(()) => break,
                Err(std::sync::mpsc::TrySendError::Full(j)) => {
                    if queued_since.elapsed() > QUEUE_TIMEOUT {
                        return Err(ScreenError::Unavailable(
                            "The previous UI operation has not finished. No new operation was sent; wait for the target app to respond or restart Muse."
                                .to_string(),
                        ));
                    }
                    job = j;
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                    return Err(ScreenError::Uia(
                        "UI Automation worker stopped.".to_string(),
                    ));
                }
            }
        }
        match rx.recv_timeout(OPERATION_TIMEOUT) {
            Ok(result) => result,
            Err(_) => {
                // Hung provider: revoke control, like .NET's `stop()`.
                self.disarm();
                Err(ScreenError::Timeout)
            }
        }
    }

    pub fn execute(&self, action: ScreenAction, args: ScreenArgs) -> ScreenResult<ScreenOutcome> {
        self.submit(|reply| Job::Execute {
            action,
            args,
            reply,
        })
    }

    pub fn authorize(&self, target: DesktopWindow) -> ScreenResult<()> {
        self.submit(|reply| Job::Authorize { target, reply })
    }

    pub fn try_auto_authorize(&self, preferred_hint: Option<&str>) -> ScreenResult<bool> {
        let hint = preferred_hint.map(|s| s.to_string());
        self.submit(|reply| Job::AutoAuthorize { hint, reply })
    }

    pub fn list_windows(&self) -> ScreenResult<Vec<DesktopWindow>> {
        self.submit(|reply| Job::ListWindows { reply })
    }

    pub fn preview(&self) -> ScreenResult<Option<String>> {
        self.submit(|reply| Job::Preview { reply })
    }

    /// Revoke control synchronously (`.NET`: `Disarm`).
    pub fn disarm(&self) {
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut t) = self.shared.target.lock() {
            *t = None;
        }
    }
}

/// Register the Esc emergency-stop hotkey on a host window
/// (`.NET`: `DesktopComputer.RegisterEmergencyStop`).
pub fn register_emergency_stop(window_handle: i64) -> bool {
    native::register_emergency_stop(window_handle)
}

/// Unregister the Esc emergency-stop hotkey
/// (`.NET`: `DesktopComputer.UnregisterEmergencyStop`).
pub fn unregister_emergency_stop(window_handle: i64) {
    native::unregister_emergency_stop(window_handle)
}

fn worker_main(shared: Arc<Shared>, rx: Receiver<Job>, init_tx: Sender<ScreenResult<()>>) {
    let automation: ScreenResult<IUIAutomation> = (|| {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(|e| ScreenError::Uia(format!("CoInitializeEx failed: {e}")))?;
            native::set_thread_dpi_awareness();
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_ALL).map_err(|e| {
                    ScreenError::Uia(format!("Could not create UIAutomation: {e}"))
                })?;
            Ok(automation)
        }
    })();
    let automation = match automation {
        Ok(a) => a,
        Err(e) => {
            let _ = init_tx.send(Err(e));
            return;
        }
    };
    let mut state = UiState {
        generation: shared.generation.load(Ordering::SeqCst),
        shared: Arc::clone(&shared),
        automation,
        elements: HashMap::new(),
        identities: HashMap::new(),
        inspected_at: None,
        captured_at: None,
        snapshot: String::new(),
        image_map: None,
        capture_window: HWND::default(),
    };
    let _ = init_tx.send(Ok(()));
    for job in rx {
        match job {
            Job::Execute {
                action,
                args,
                reply,
            } => {
                let _ = reply.send(uia::execute_action(&mut state, action, args));
            }
            Job::Authorize { target, reply } => {
                let _ = reply.send(authorize(&mut state, target));
            }
            Job::AutoAuthorize { hint, reply } => {
                let _ = reply.send(auto_authorize(&mut state, hint.as_deref()));
            }
            Job::ListWindows { reply } => {
                let mut wins = native::enumerate_windows();
                wins.sort_by(|a, b| a.title.cmp(&b.title));
                let _ = reply.send(Ok(wins));
            }
            Job::Preview { reply } => {
                let _ = reply.send(preview(&mut state));
            }
        }
    }
}

/// Authorize a target window (`.NET`: `DesktopComputer.Authorize`).
fn authorize(state: &mut UiState, target: DesktopWindow) -> ScreenResult<()> {
    let handle = native::hwnd(target.handle);
    if !native::is_window(handle) {
        return Err(ScreenError::WindowClosed);
    }
    let pid = native::window_process_id(handle);
    if pid != target.process_id as u32 || pid == std::process::id() {
        return Err(ScreenError::InvalidInput(
            "The window is closed or belongs to Muse itself. Refresh the list and choose again."
                .to_string(),
        ));
    }
    let started = native::process_start_ticks(pid).ok_or(ScreenError::WindowClosed)?;
    if target.process_started != 0 && target.process_started != started {
        return Err(ScreenError::InvalidInput(
            "The target app restarted. Please choose the window again.".to_string(),
        ));
    }
    // Disarm first (bumps the generation), then install the new target.
    state.shared.generation.fetch_add(1, Ordering::SeqCst);
    state.generation = state.shared.generation.load(Ordering::SeqCst);
    state.elements.clear();
    state.identities.clear();
    state.snapshot.clear();
    state.image_map = None;
    let target = DesktopWindow {
        process_started: started,
        ..target
    };
    if let Ok(mut t) = state.shared.target.lock() {
        *t = Some(target);
    }
    Ok(())
}

/// Auto-select a target window (`.NET`: `DesktopComputer.TryAutoAuthorize`).
fn auto_authorize(state: &mut UiState, preferred_hint: Option<&str>) -> ScreenResult<bool> {
    if uia::lock_target(state).is_some() {
        return Ok(true);
    }
    let windows = native::enumerate_windows();
    let foreground = native::foreground_window();
    let mut foreground_handle = foreground.0 as i64;
    if foreground.0.is_null() {
        foreground_handle = 0;
    } else if windows.iter().all(|w| w.handle != foreground_handle) {
        let self_pid = std::process::id();
        let owner = windows.iter().find(|w| {
            w.process_id != self_pid as i32
                && native::in_scope(native::hwnd(w.handle), foreground)
        });
        if let Some(owner) = owner {
            foreground_handle = owner.handle;
        }
    }
    let pick = WindowAutoPicker::pick(
        &windows,
        foreground_handle,
        std::process::id() as i32,
        preferred_hint,
    );
    match pick {
        None => Ok(false),
        Some(w) => match authorize(state, w) {
            Ok(()) => Ok(true),
            Err(_) => Ok(false),
        },
    }
}

/// Live preview without focusing or replacing the snapshot
/// (`.NET`: `DesktopComputer.PreviewAsync`).
fn preview(state: &mut UiState) -> ScreenResult<Option<String>> {
    let Some(target) = uia::lock_target(state) else {
        return Ok(None);
    };
    let outcome = shot::capture(state, &target, true)?;
    Ok(outcome.image_data_url)
}
