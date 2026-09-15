use crate::edit::EditSession;
use crate::fwd::Forward;
use crate::ops::XferHandle;
use crate::term::TermSession;
use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::Mutex;

#[derive(Default)]
pub struct AppState {
    pub terms: Mutex<HashMap<u64, TermSession>>,
    pub transfers: Mutex<HashMap<u64, XferHandle>>,
    pub edits: Mutex<HashMap<u64, EditSession>>,
    pub fwds: Mutex<HashMap<u64, Forward>>,
    pub next_id: AtomicU64,
}
