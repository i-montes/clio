//! Raising the cost of the passphrase goes through the person's socket only, and the status says
//! which cost a vault has.
#![allow(clippy::unwrap_used)]

use clio_core::SystemClock;
use clio_vault::Vault;
use clio_vault::proto::Request;
use clio_vault::proto::Side;
use clio_vault::server::handle;
use std::sync::Arc;

fn upgrade(passphrase: &str) -> Request {
    Request::UpgradeKdf {
        passphrase: passphrase.into(),
    }
}

#[test]
fn only_the_person_socket_can_raise_the_cost() {
    let (v, _) = Vault::init(None, "una frase larga de prueba", Arc::new(SystemClock)).unwrap();
    let r = handle(&v, Side::Gateway, upgrade("una frase larga de prueba"));
    assert!(!r.ok, "{:?}", r.error);
    assert!(r.error.unwrap().contains("not allowed"));
    // On the person's socket a current vault has nothing to do; a wrong passphrase is refused.
    let r = handle(&v, Side::Person, upgrade("una frase larga de prueba"));
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.data.unwrap()["upgraded"], false);
    let r = handle(&v, Side::Person, upgrade("otra frase"));
    assert!(!r.ok, "{:?}", r.error);
}

#[test]
fn the_status_says_the_cost_of_the_vault() {
    let (v, _) = Vault::init(None, "una frase larga de prueba", Arc::new(SystemClock)).unwrap();
    for side in [Side::Person, Side::Gateway] {
        let d = handle(&v, side, Request::Status).data.unwrap();
        assert_eq!(d["kdf_current"], true, "{d}");
        assert_eq!(d["kdf"], "argon2id m=64 MiB t=3 p=1", "{d}");
    }
}
