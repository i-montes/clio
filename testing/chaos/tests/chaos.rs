use clio_chaos::{Behavior, FAULTS, Fault, System, run};

struct Fixed(fn(Fault) -> Behavior);
impl System for Fixed {
    fn inject(&mut self, fault: Fault) -> Behavior {
        (self.0)(fault)
    }
}

#[test]
fn a_system_that_fails_closed_everywhere_passes() {
    let r = run(&mut Fixed(|_| Behavior::FailedClosed));
    assert!(r.ok());
    assert_eq!(r.safe.len(), FAULTS.len());
}

#[test]
fn corruption_or_a_leak_under_any_fault_blocks_the_gate() {
    let r = run(&mut Fixed(|f| match f {
        Fault::DiskFull => Behavior::Corrupted,
        Fault::VaultLocked => Behavior::Leaked,
        _ => Behavior::Degraded,
    }));
    assert!(!r.ok());
    assert_eq!(
        r.unsafe_,
        [
            (Fault::DiskFull, Behavior::Corrupted),
            (Fault::VaultLocked, Behavior::Leaked)
        ]
    );
}

#[test]
fn a_fault_with_nothing_to_test_is_pending_never_safe() {
    let r = run(&mut Fixed(|_| Behavior::Unknown));
    assert!(r.ok());
    assert!(r.safe.is_empty());
    assert_eq!(r.pending.len(), FAULTS.len());
}
