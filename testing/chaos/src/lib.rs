//! Chaos harness: break one thing at a time and check the system fails closed instead of
//! corrupting data or leaking. A fault whose system is not built yet reports `Unknown`, which is
//! listed as pending and never counted as safe.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Fault {
    NetworkCut,
    VpsDown,
    DiskFull,
    ProviderDown,
    VaultLocked,
}

pub const FAULTS: [Fault; 5] = [
    Fault::NetworkCut,
    Fault::VpsDown,
    Fault::DiskFull,
    Fault::ProviderDown,
    Fault::VaultLocked,
];

/// What the system did while the fault was in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Behavior {
    /// It stopped the affected work and said so; nothing was lost.
    FailedClosed,
    /// It kept doing the safe part (reversible work, queued messages).
    Degraded,
    Corrupted,
    Leaked,
    /// Nothing to test yet.
    Unknown,
}

impl Behavior {
    pub fn safe(self) -> bool {
        matches!(self, Self::FailedClosed | Self::Degraded)
    }
}

pub trait System {
    /// Applies `fault`, observes, and puts everything back.
    fn inject(&mut self, fault: Fault) -> Behavior;
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub safe: Vec<Fault>,
    pub unsafe_: Vec<(Fault, Behavior)>,
    pub pending: Vec<Fault>,
}

impl Report {
    /// The gate: no fault ended in corruption or a leak.
    pub fn ok(&self) -> bool {
        self.unsafe_.is_empty()
    }

    pub fn line(&self) -> String {
        format!(
            "chaos: {} safe · {} unsafe · {} pending",
            self.safe.len(),
            self.unsafe_.len(),
            self.pending.len()
        )
    }
}

pub fn run(system: &mut dyn System) -> Report {
    let mut report = Report::default();
    for fault in FAULTS {
        match system.inject(fault) {
            Behavior::Unknown => report.pending.push(fault),
            b if b.safe() => report.safe.push(fault),
            b => report.unsafe_.push((fault, b)),
        }
    }
    report
}
