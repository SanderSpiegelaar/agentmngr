use std::sync::mpsc::{self, Receiver, Sender};

use crate::Error;

#[derive(Debug)]
pub enum CoreEvent {
    ItemsChanged { ids: Vec<String> },
    PlanReady,
    ApplyDone,
    SyncState { state: String },
    UpdateFound { ids: Vec<String> },
    Error(Error),
}

/// Workers clone the sender; the GUI bridge owns the single receiver.
pub fn channel() -> (Sender<CoreEvent>, Receiver<CoreEvent>) {
    mpsc::channel()
}

#[cfg(test)]
mod tests {
    use super::{CoreEvent, channel};
    use crate::Error;

    #[test]
    fn delivers_worker_events_and_reports_disconnect() {
        let (sender, receiver) = channel();
        let worker_sender = sender.clone();
        let worker = std::thread::spawn(move || {
            for event in [
                CoreEvent::ItemsChanged {
                    ids: vec!["skill".into()],
                },
                CoreEvent::PlanReady,
                CoreEvent::ApplyDone,
                CoreEvent::SyncState {
                    state: "idle".into(),
                },
                CoreEvent::UpdateFound {
                    ids: vec!["skill".into()],
                },
                CoreEvent::Error(Error::ConfigChanged("reload".into())),
            ] {
                worker_sender.send(event).unwrap();
            }
        });
        drop(sender);
        let events: Vec<_> = receiver.iter().collect();
        worker.join().unwrap();
        assert_eq!(events.len(), 6);
        assert!(matches!(&events[0], CoreEvent::ItemsChanged { ids } if ids == &["skill"]));
        assert!(matches!(events[1], CoreEvent::PlanReady));
        assert!(matches!(events[2], CoreEvent::ApplyDone));
        assert!(matches!(&events[3], CoreEvent::SyncState { state } if state == "idle"));
        assert!(matches!(&events[4], CoreEvent::UpdateFound { ids } if ids == &["skill"]));
        assert!(
            matches!(&events[5], CoreEvent::Error(error) if error.code() == "E_CONFIG_CHANGED")
        );
        assert!(receiver.recv().is_err());

        let (sender, receiver) = channel();
        drop(receiver);
        assert!(sender.send(CoreEvent::ApplyDone).is_err());
    }
}
