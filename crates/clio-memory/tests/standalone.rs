//! Decision 0001: clio-memory depends on no other Clio crate, so it can move to its own repo.

/// Names of the `clio-*` crates listed under any dependencies table, or a `[dependencies.clio-*]` table.
fn clio_dependencies(manifest: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut in_deps = false;
    for line in manifest.lines().map(str::trim) {
        if let Some(table) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_deps = table.ends_with("dependencies");
            if let Some((_, name)) = table.rsplit_once("dependencies.") {
                found.push(name.to_string());
            }
        } else if in_deps && let Some((key, _)) = line.split_once('=') {
            // `clio-core.workspace = true` names the crate before the first dot
            found.push(key.trim().split('.').next().unwrap_or_default().to_string());
        }
    }
    found.retain(|name| name.starts_with("clio-"));
    found
}

#[test]
fn clio_memory_depends_on_no_other_clio_crate() {
    let manifest =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
    assert_eq!(clio_dependencies(&manifest), Vec::<String>::new());
}

#[test]
fn the_check_sees_each_way_of_declaring_a_clio_dependency() {
    let inline = "[dependencies]\nserde = \"1\"\nclio-trace = { path = \"../clio-trace\" }\n";
    assert_eq!(clio_dependencies(inline), ["clio-trace"]);
    let dev = "[dev-dependencies]\nclio-core.workspace = true\n";
    assert_eq!(clio_dependencies(dev), ["clio-core"]);
    let table = "[dependencies.clio-vault]\npath = \"../clio-vault\"\n";
    assert_eq!(clio_dependencies(table), ["clio-vault"]);
    let clean = "[package]\nname = \"clio-memory\"\n[dependencies]\nserde = \"1\"\n";
    assert!(clio_dependencies(clean).is_empty());
}
