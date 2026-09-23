//! Private collection ownership; never part of semantic snapshots or cursors.
use crate::store::PrOwner;
use std::{cell::RefCell, future::Future};

tokio::task_local! { static OWNER: RefCell<Option<PrOwner>>; }

pub(crate) async fn scope<T>(future: impl Future<Output = T>) -> T {
    OWNER.scope(RefCell::new(None), future).await
}

pub(crate) fn current() -> Option<PrOwner> {
    OWNER
        .try_with(|owner| owner.borrow().clone())
        .ok()
        .flatten()
}

pub(crate) fn set(owner: PrOwner) {
    let _ = OWNER.try_with(|slot| *slot.borrow_mut() = Some(owner));
}

pub(crate) fn clear() {
    let _ = OWNER.try_with(|slot| *slot.borrow_mut() = None);
}

pub(crate) fn changed() -> crate::Error {
    crate::Error::Invalid("PR entity changed while collecting evidence".into())
}
