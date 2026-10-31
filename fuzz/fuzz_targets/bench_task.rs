#![no_main]
use libfuzzer_sys::fuzz_target;

// A task file is read from disk: any bytes must give a task or an error, never a panic.
fuzz_target!(|data: &[u8]| {
    if let Ok(task) = serde_json::from_slice::<clio_bench::Task>(data) {
        let _ = task.check.passes("respuesta");
    }
});
