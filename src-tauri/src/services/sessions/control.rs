//! SessionControl：停止/接管/取消的原子旗标集（纯搬移自 mod.rs）。

use super::*;

/// Flags stop / takeover can set without waiting for `SessionService`.
pub struct SessionControl {
    pub(super) cancel: AtomicBool,
    pub(super) stop_requested: AtomicBool,
    pub(super) stop_tts: AtomicBool,
    pub(super) mode: AtomicU8,
    pub(super) session_id: Mutex<Option<String>>,
    pub(super) confirmation_epoch: AtomicU64,
    pub(super) barge_in_source: Mutex<Option<Arc<AtomicBool>>>,
}

impl SessionControl {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            cancel: AtomicBool::new(false),
            stop_requested: AtomicBool::new(false),
            stop_tts: AtomicBool::new(false),
            mode: AtomicU8::new(mode_u8(AgentMode::AiActive)),
            session_id: Mutex::new(None),
            confirmation_epoch: AtomicU64::new(0),
            barge_in_source: Mutex::new(None),
        })
    }

    pub fn request_cancel(&self) {
        self.confirmation_epoch.fetch_add(1, Ordering::SeqCst);
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn request_stop(&self) {
        self.confirmation_epoch.fetch_add(1, Ordering::SeqCst);
        self.stop_requested.store(true, Ordering::SeqCst);
        self.cancel.store(true, Ordering::SeqCst);
        self.stop_tts.store(true, Ordering::SeqCst);
    }

    pub fn set_mode(&self, mode: AgentMode) {
        self.mode.store(mode_u8(mode), Ordering::SeqCst);
        if mode == AgentMode::AiActive {
            self.stop_tts.store(false, Ordering::SeqCst);
        } else {
            self.confirmation_epoch.fetch_add(1, Ordering::SeqCst);
            self.stop_tts.store(true, Ordering::SeqCst);
            self.cancel.store(true, Ordering::SeqCst);
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    pub fn stop_requested(&self) -> bool {
        self.stop_requested.load(Ordering::SeqCst)
    }

    pub fn mode(&self) -> AgentMode {
        mode_from_u8(self.mode.load(Ordering::SeqCst))
    }

    pub fn session_id(&self) -> Option<String> {
        self.session_id.lock().ok().and_then(|guard| guard.clone())
    }

    pub fn take_stop_tts(&self) -> bool {
        self.stop_tts.swap(false, Ordering::SeqCst)
    }

    pub fn set_barge_in_source(&self, flag: Arc<AtomicBool>) {
        *self
            .barge_in_source
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(flag);
    }
    pub fn barge_in_requested(&self) -> bool {
        self.barge_in_source
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
    }
    pub fn take_barge_in(&self) -> bool {
        self.barge_in_source
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .is_some_and(|flag| flag.swap(false, Ordering::SeqCst))
    }

    pub(super) fn cancel_flag(&self) -> &AtomicBool {
        &self.cancel
    }

    pub(super) fn clear_cancel(&self) {
        self.cancel.store(false, Ordering::SeqCst);
    }

    pub(super) fn set_session_id(&self, session_id: Option<String>) {
        if let Ok(mut guard) = self.session_id.lock() {
            *guard = session_id;
        }
    }

    pub(super) fn reset(&self) {
        self.cancel.store(false, Ordering::SeqCst);
        self.stop_requested.store(false, Ordering::SeqCst);
        self.stop_tts.store(false, Ordering::SeqCst);
        self.mode
            .store(mode_u8(AgentMode::AiActive), Ordering::SeqCst);
        // 丢弃 barge 旗标来源：停止/重建会话后，旧 capture 的旗标不得继续作用于控制端。
        *self
            .barge_in_source
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = None;
        self.set_session_id(None);
    }
}
