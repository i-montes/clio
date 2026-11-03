#![no_main]
use libfuzzer_sys::fuzz_target;

// Every line Pi writes goes through `Event::from_value`: any JSON must give an event, never a
// panic (an unknown or malformed record is `Unknown`/defaults).
fuzz_target!(|data: &[u8]| {
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(data) {
        let _ = clio_brain::pi::rpc::Event::from_value(&v);
    }
});
