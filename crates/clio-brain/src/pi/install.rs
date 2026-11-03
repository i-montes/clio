//! Installing and finding Pi. The version comes from `adapters/pi/package.json`, embedded in the
//! binary together with `package-lock.json`, so a `clio` copied to `~/.local/bin` installs the
//! same Pi without the repository. Install = `npm ci --ignore-scripts` from the embedded
//! lockfile (never `npm install`, never install scripts: S-42).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::{PiError, PiPaths, io_err, private_dir, text, write_private};

pub const PACKAGE_JSON: &str = include_str!("../../../../adapters/pi/package.json");
pub const PACKAGE_LOCK: &str = include_str!("../../../../adapters/pi/package-lock.json");
pub const PI_PACKAGE: &str = "@earendil-works/pi-coding-agent";
/// Pi needs Node 22.19 or newer.
pub const MIN_NODE: (u32, u32, u32) = (22, 19, 0);

/// The pinned Pi version. It must be exact (`1.4.0`, never `^1.4.0`, `latest` or a range).
pub fn pinned_version() -> Result<String, PiError> {
    let v: serde_json::Value = serde_json::from_str(PACKAGE_JSON)
        .map_err(|e| PiError::Io(format!("package.json incrustado inválido: {e}")))?;
    let ver = v["dependencies"][PI_PACKAGE]
        .as_str()
        .ok_or_else(|| PiError::Io(format!("package.json no fija {PI_PACKAGE}")))?;
    if parse_node_version(ver).is_none() || ver.starts_with('v') {
        return Err(PiError::Io(format!(
            "la versión de Pi debe ser exacta (x.y.z), no {ver:?}"
        )));
    }
    Ok(ver.to_owned())
}

/// `v22.23.2` or `22.23.2` to `(22, 23, 2)`.
pub fn parse_node_version(text: &str) -> Option<(u32, u32, u32)> {
    let mut it = text.trim().trim_start_matches('v').split('.');
    let n = |s: Option<&str>| s?.parse::<u32>().ok();
    let r = (n(it.next())?, n(it.next())?, n(it.next())?);
    it.next().is_none().then_some(r)
}

#[derive(Debug, Clone)]
pub struct NodeInfo {
    pub path: PathBuf,
    pub version: (u32, u32, u32),
}

impl NodeInfo {
    pub fn dir(&self) -> PathBuf {
        self.path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default()
    }
}

fn find_in_path(name: &str, path_var: &str) -> Option<PathBuf> {
    path_var
        .split(':')
        .filter(|d| !d.is_empty())
        .map(|d| Path::new(d).join(name))
        .find(|p| {
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// Node from `path_var` (pass `$PATH`), at least [`MIN_NODE`]. The error says what to do.
pub fn find_node(path_var: &str) -> Result<NodeInfo, PiError> {
    let path = find_in_path("node", path_var).ok_or_else(|| {
        PiError::Node(
            "no encontré Node en el PATH. Pi lo necesita (22.19 o más): instálalo (por ejemplo con tu gestor de paquetes o mise) y vuelve a intentar".into(),
        )
    })?;
    let out = Command::new(&path)
        .arg("--version")
        .output()
        .map_err(|e| PiError::Node(format!("no pude ejecutar {}: {e}", path.display())))?;
    let raw = String::from_utf8_lossy(&out.stdout);
    let version = parse_node_version(&raw).ok_or_else(|| {
        PiError::Node(format!(
            "{} --version dio algo inesperado: {}",
            path.display(),
            text::line(&raw, 40)
        ))
    })?;
    if version < MIN_NODE {
        return Err(PiError::Node(format!(
            "Node {}.{}.{} es muy viejo: Pi necesita 22.19 o más. Actualiza Node (mise, nvm o tu gestor de paquetes)",
            version.0, version.1, version.2
        )));
    }
    Ok(NodeInfo { path, version })
}

#[derive(Debug, Clone)]
pub struct PiInstall {
    pub version: String,
    pub dir: PathBuf,
    /// `<dir>/node_modules/.bin/pi`
    pub bin: PathBuf,
}

/// The version found inside an install folder, if any.
pub fn installed_version(dir: &Path) -> Option<String> {
    let p = dir
        .join("node_modules")
        .join(PI_PACKAGE)
        .join("package.json");
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()?;
    v["version"].as_str().map(str::to_owned)
}

/// The install of the pinned version, if it is there and is that version.
pub fn locate(paths: &PiPaths) -> Result<PiInstall, PiError> {
    let version = pinned_version()?;
    let dir = paths.install_dir(&version);
    let bin = dir.join("node_modules/.bin/pi");
    let hint = "ejecuta `clio init` (o `clio pi install`)";
    if !std::fs::metadata(&bin).is_ok_and(|m| m.permissions().mode() & 0o111 != 0) {
        return Err(PiError::NotInstalled(format!(
            "Pi {version} no está instalado en {}: {hint}",
            dir.display()
        )));
    }
    match installed_version(&dir) {
        Some(v) if v == version => Ok(PiInstall { version, dir, bin }),
        other => Err(PiError::NotInstalled(format!(
            "en {} hay Pi {} y Clio fija {version}: borra esa carpeta y {hint}",
            dir.display(),
            other.as_deref().unwrap_or("desconocido")
        ))),
    }
}

/// Every folder 0700, every file without group/other access (keeps the owner's exec bit).
fn lock_down(dir: &Path) -> Result<(), PiError> {
    let rd = std::fs::read_dir(dir).map_err(|e| io_err(dir, &e))?;
    for e in rd.filter_map(Result::ok) {
        let p = e.path();
        let Ok(m) = std::fs::symlink_metadata(&p) else {
            continue;
        };
        if m.is_symlink() {
            continue; // set_permissions would follow it
        }
        if m.is_dir() {
            lock_down(&p)?;
        } else {
            let mode = m.permissions().mode() & 0o700;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode))
                .map_err(|e| io_err(&p, &e))?;
        }
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| io_err(dir, &e))
}

/// Installs the pinned Pi (a no-op when it is already there). Needs the network the first time.
/// Installs into a temporary sibling folder and renames it, so a failed install leaves nothing
/// that looks installed.
pub fn install(paths: &PiPaths, path_var: &str) -> Result<PiInstall, PiError> {
    if let Ok(i) = locate(paths) {
        return Ok(i);
    }
    let node = find_node(path_var)?;
    let npm = find_in_path("npm", path_var).ok_or_else(|| {
        PiError::Node("no encontré npm junto a Node: instala npm y vuelve a intentar".into())
    })?;
    let version = pinned_version()?;
    paths.ensure()?;
    let dir = paths.install_dir(&version);
    let tmp = paths.root.join(format!(".install-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let result = (|| {
        private_dir(&tmp)?;
        write_private(&tmp.join("package.json"), PACKAGE_JSON.as_bytes())?;
        write_private(&tmp.join("package-lock.json"), PACKAGE_LOCK.as_bytes())?;
        write_private(
            &tmp.join(".npmrc"),
            b"ignore-scripts=true\naudit=false\nfund=false\n",
        )?;
        run_npm_ci(&npm, &node, &tmp)?;
        lock_down(&tmp)?;
        let _ = std::fs::remove_dir_all(&dir); // a half-broken earlier folder
        std::fs::rename(&tmp, &dir).map_err(|e| io_err(&dir, &e))
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    locate(paths)
}

fn run_npm_ci(npm: &Path, node: &NodeInfo, cwd: &Path) -> Result<(), PiError> {
    let mut child = Command::new(npm)
        .args([
            "ci",
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
            "--loglevel=error",
        ])
        .current_dir(cwd)
        .env("PATH", format!("{}:/usr/bin:/bin", node.dir().display()))
        .env("npm_config_ignore_scripts", "true")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| PiError::Spawn(format!("no pude ejecutar npm: {e}")))?;
    let mut stderr = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(s) = stderr.as_mut() {
            let _ = std::io::Read::read_to_end(&mut std::io::Read::take(s, 64 * 1024), &mut buf);
        }
        String::from_utf8_lossy(&buf).into_owned()
    });
    let deadline = Instant::now() + Duration::from_secs(600);
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() > deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(PiError::Timeout {
                    what: "npm ci".into(),
                    secs: 600,
                });
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(PiError::Spawn(format!("npm: {e}"))),
        }
    };
    let err = reader.join().unwrap_or_default();
    if status.success() {
        Ok(())
    } else {
        Err(PiError::NotInstalled(format!(
            "`npm ci` falló ({status}); revisa la red y vuelve a intentar. {}",
            text::block(&err, 600)
        )))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_pinned_version_comes_from_the_embedded_package_json() {
        let v = pinned_version().unwrap();
        assert!(parse_node_version(&v).is_some(), "{v}");
        assert!(PACKAGE_LOCK.contains(PI_PACKAGE));
    }

    #[test]
    fn node_versions_parse_and_compare() {
        assert_eq!(parse_node_version("v22.23.2\n"), Some((22, 23, 2)));
        assert_eq!(parse_node_version("^1.4.0"), None);
        assert_eq!(parse_node_version("1.4"), None);
        assert!((22, 18, 9) < MIN_NODE && (22, 19, 0) >= MIN_NODE && (23, 0, 0) >= MIN_NODE);
    }

    #[test]
    fn locate_asks_for_install_when_missing_and_rejects_another_version() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = PiPaths {
            root: tmp.path().to_owned(),
        };
        let e = locate(&paths).unwrap_err().to_string();
        assert!(
            e.contains("no está instalado") && e.contains("clio init"),
            "{e}"
        );
        let dir = paths.install_dir(&pinned_version().unwrap());
        std::fs::create_dir_all(dir.join("node_modules/.bin")).unwrap();
        std::fs::create_dir_all(dir.join("node_modules").join(PI_PACKAGE)).unwrap();
        std::fs::write(dir.join("node_modules/.bin/pi"), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(
            dir.join("node_modules/.bin/pi"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        std::fs::write(
            dir.join("node_modules")
                .join(PI_PACKAGE)
                .join("package.json"),
            r#"{"version":"0.0.1"}"#,
        )
        .unwrap();
        assert!(
            locate(&paths)
                .unwrap_err()
                .to_string()
                .contains("hay Pi 0.0.1")
        );
    }

    #[test]
    fn a_missing_node_is_an_actionable_error() {
        let e = find_node("/nonexistent-dir").unwrap_err().to_string();
        assert!(e.contains("Node") && e.contains("22.19"), "{e}");
    }
}
