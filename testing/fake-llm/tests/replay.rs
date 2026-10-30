use clio_fake_llm::*;

fn req(text: &str) -> Request {
    Request {
        model: "m-1".into(),
        messages: vec![Message {
            role: "user".into(),
            content: text.into(),
        }],
        tools: vec![],
        max_tokens: Some(100),
    }
}

fn answer(text: &str) -> Response {
    Response {
        content: text.into(),
        input_tokens: 3,
        output_tokens: 2,
    }
}

fn tape_path(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("clio-fake-llm-{name}-{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p
}

#[test]
fn two_replays_give_exactly_the_same_result() {
    let path = tape_path("twice");
    let rec = Record::new(
        Scripted::new([answer("hola")]),
        Tape::open(&path).expect("tape"),
    );
    rec.complete(&req("saluda")).expect("recorded");

    let run = || {
        let replay = Replay::new(Tape::open(&path).expect("tape"));
        replay.complete(&req("saluda")).expect("replayed")
    };
    assert_eq!(run(), run());
    assert_eq!(run(), answer("hola"));
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_request_with_no_recording_fails_with_its_hash() {
    let replay = Replay::new(Tape::in_memory());
    let err = replay.complete(&req("nuevo")).expect_err("must fail");
    assert!(matches!(&err, Error::NotRecorded { hash } if *hash == request_hash(&req("nuevo"))));
}

#[test]
fn the_hash_ignores_the_order_of_fields_in_the_json() {
    let a: Request =
        serde_json::from_str(r#"{"model":"m-1","messages":[],"tools":[],"max_tokens":1}"#)
            .expect("a");
    let b: Request =
        serde_json::from_str(r#"{"max_tokens":1,"tools":[],"messages":[],"model":"m-1"}"#)
            .expect("b");
    assert_eq!(request_hash(&a), request_hash(&b));
    assert_ne!(request_hash(&req("x")), request_hash(&req("y")));
}

#[test]
fn recording_stops_a_secret_and_writes_nothing() {
    let path = tape_path("secret");
    let rec = Record::new(
        Scripted::new([answer("la llave es sk-abcdefghijklmnopqrstuvwxyz")]),
        Tape::open(&path).expect("tape"),
    );
    assert!(matches!(
        rec.complete(&req("dame la llave")),
        Err(Error::Secret { .. })
    ));
    assert!(!path.exists() || std::fs::read_to_string(&path).expect("read").is_empty());
    let _ = std::fs::remove_file(path);
}

#[test]
fn record_replays_what_it_already_has_without_asking_again() {
    let rec = Record::new(Scripted::new([answer("una")]), Tape::in_memory());
    let first = rec.complete(&req("q")).expect("first");
    let second = rec
        .complete(&req("q"))
        .expect("second, from the tape: the script is empty");
    assert_eq!(first, second);
}

#[test]
fn scripted_runs_out() {
    let s = Scripted::new([answer("a")]);
    s.complete(&req("q")).expect("one");
    assert!(matches!(s.complete(&req("q")), Err(Error::ScriptExhausted)));
}
