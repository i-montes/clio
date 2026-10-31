#![no_main]
use libfuzzer_sys::fuzz_target;

// A parser that takes text from another node: it must never panic, and what it accepts must
// print back to something that parses to the same value.
fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        if let Ok(parsed) = clio_trace::Traceparent::parse(text) {
            let again = clio_trace::Traceparent::parse(&parsed.to_string()).expect("round trip");
            assert_eq!(parsed, again);
        }
    }
});
