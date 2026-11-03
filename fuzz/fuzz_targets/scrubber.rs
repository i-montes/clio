#![no_main]
use std::sync::Arc;

use libfuzzer_sys::fuzz_target;

// The vault's output filter sees whatever a tool printed. Any text must come out without a panic,
// and a value it knows must never survive, whatever surrounds it.
fuzz_target!(|data: &[u8]| {
    const KNOWN: &str = "violeta-azul-7731-clio";
    let scrubber = clio_vault::Scrubber::new(Arc::new(clio_core::SystemClock));
    scrubber.add("known", KNOWN);
    let text = String::from_utf8_lossy(data);
    let _ = scrubber.scrub(&text);
    let planted = format!("{text} {KNOWN} {text}");
    assert!(!scrubber.scrub(&planted).contains(KNOWN));
});
