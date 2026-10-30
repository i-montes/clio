use clio_canaries::*;

fn b64(bytes: &[u8]) -> String {
    // Independent of the crate's own encoder: a table-free check through known vectors below.
    let t = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                s.push(t[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

#[test]
fn the_same_seed_gives_the_same_decoys_and_different_seeds_differ() {
    let (a, b, c) = (
        Canaries::from_seed(1),
        Canaries::from_seed(1),
        Canaries::from_seed(2),
    );
    assert_eq!(a.value("notion"), b.value("notion"));
    assert_ne!(a.value("notion"), c.value("notion"));
    assert_ne!(a.value("notion"), a.value("github"));
}

#[test]
fn a_plain_decoy_in_a_trace_is_found() {
    let c = Canaries::from_seed(7);
    let value = c.value("notion").expect("notion");
    let leaks = c.scan("trace", &format!("span args={{\"token\":\"{value}\"}}"));
    assert_eq!(leaks.len(), 1);
    assert_eq!(
        (leaks[0].name.as_str(), leaks[0].form),
        ("notion", Form::Plain)
    );
}

#[test]
fn base64_at_any_alignment_and_url_encoding_are_found() {
    let c = Canaries::from_seed(7);
    let value = c.value("ssh").expect("ssh");
    for pad in 0..3 {
        let mut bytes = vec![b'#'; pad];
        bytes.extend_from_slice(value.as_bytes());
        let leaks = c.scan("log", &format!("blob {} end", b64(&bytes)));
        assert!(
            leaks
                .iter()
                .any(|l| l.name == "ssh" && l.form == Form::Base64),
            "pad {pad}"
        );
    }
    let encoded: String = value.bytes().map(|b| format!("%{b:02X}")).collect();
    assert!(
        c.scan("url", &format!("https://x.test/?k={encoded}"))
            .iter()
            .any(|l| l.form == Form::UrlEncoded)
    );
}

#[test]
fn clean_text_has_no_leaks() {
    let c = Canaries::from_seed(7);
    assert!(
        c.scan(
            "trace",
            "sk-canary- is not a full decoy, and neither is this"
        )
        .is_empty()
    );
    c.assert_clean("trace", "nothing here");
}

#[test]
#[should_panic(expected = "canary \"github\" leaked")]
fn a_decoy_in_a_trace_fails_the_gate() {
    let c = Canaries::from_seed(7);
    let value = c.value("github").expect("github");
    c.assert_clean("trace", &format!("calling with {value}"));
}

#[test]
fn scan_dir_finds_a_decoy_in_a_nested_file() {
    let c = Canaries::from_seed(7);
    let dir = std::env::temp_dir().join(format!("clio-canaries-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("a/b")).expect("dirs");
    std::fs::write(dir.join("a/ok.log"), "fine").expect("write");
    std::fs::write(
        dir.join("a/b/bad.log"),
        format!("x {} y", c.value("llm-provider").expect("v")),
    )
    .expect("write");
    let leaks = c.scan_dir(&dir).expect("scan");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(leaks.len(), 1);
    assert!(leaks[0].place.ends_with("bad.log"));
}
