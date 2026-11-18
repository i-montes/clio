# Manos reales (modo anfitrión) — plan

> **Para agentes:** se ejecuta tarea por tarea, con TDD y un commit por tarea. Pasos con casillas.

**Objetivo:** las herramientas de Clio actúan sobre la máquina real, con red y con `sudo`, bajo
pocas reglas fijas: todo queda registrado, copia antes de cambiar, y solo cuatro clases de acción
esperan el visto bueno de la persona, sin detener la tarea.

**Arquitectura:** Pi no cambia y sigue en su jaula. El toolhost gana un modo anfitrión
(`Config.host`): sus operaciones de archivos trabajan sobre `/` y `exec` corre con un lanzador nuevo
de `clio-node` que muestra el sistema real y tapa lo de Clio. Una lista (no el modelo) decide qué
se aparta. Lo apartado vive en una carpeta de pendientes que la persona aprueba desde la terminal
(y, en la parte 3, desde Telegram). El modo copia de hoy queda igual y sigue siendo el de fábrica.

**Stack:** Rust (workspace `clio`), TypeScript (la guardia de Pi). `regex` ya está en `Cargo.lock`
y se puede usar. Sin dependencias nuevas fuera de esa.

**Spec:** `docs/specs/2026-11-17-clio-asistente-autonomo-design.md`, sección 2.

## Tres cosas que el diseño no decía y este plan decide

1. **`sudo` no funciona dentro de `bwrap`** (comprobado en el VPS el 2026-11-17: `bwrap` pone
   `no_new_privs` y `sudo` se niega). El diseño pedía a la vez «jaula que tapa lo de Clio» y «`sudo`
   libre». Por eso hay dos lanzadores:
   - **`systemd`**, cuando `sudo -n true` funciona y existe `systemd-run`: la orden corre en una
     unidad transitoria del sistema, como el usuario de Clio, con lo de Clio tapado
     (`TemporaryFileSystem=`, `InaccessiblePaths=`). Dentro, `sudo` funciona. Comprobado en el VPS.
   - **`bwrap`**, cuando no hay `sudo` sin contraseña: `--bind / /` con `--tmpfs` sobre lo de Clio.
     Ahí `sudo` no funciona, y tampoco habría funcionado sin terminal.
2. **La red se pide por orden.** Una orden corre sin red salvo que lleve `network: true`. Una orden
   con red marca la sesión («web»). Así la marca la decide el núcleo y no una lista de nombres de
   programas (`curl`, `wget`…), que cualquier script se salta sin querer. Costo: revisar
   `localhost` también pide red, y marca.
3. **Preguntar en la terminal es al final del turno,** no en medio de la orden: la acción se aparta
   siempre, la tarea sigue, y al terminar el turno la interfaz pregunta por cada apartada. Es el
   mismo camino con persona y sin ella.

Fuera de este plan: que Temis juzgue cada texto ajeno (llega con las conexiones, parte 6, que es
por donde entra texto ajeno); el aviso por Telegram (parte 3); el resumen del día (parte 4).

## Restricciones globales

- **Nombres.** Modo: `host` (anfitrión) y `copy` (copia). Configuración: `"mode": "copy" | "host"`
  en `config.json`, de fábrica `copy`. Opciones `--host` y `--copy` en `clio` y `clio run`, que
  mandan sobre la configuración. Órdenes nuevas: `clio pending`, `clio approve ID`,
  `clio reject ID`, `clio actions`. `clio undo ID` sirve también para una sesión anfitrión.
- **Carpeta de trabajo del modo anfitrión:** la carpeta personal (`Dirs.home`). Una ruta relativa
  se resuelve contra ella y las órdenes arrancan ahí.
- **Lo de Clio** (lista `control`, rutas canónicas): `Dirs.config`, `Dirs.data`, `Dirs.runtime`, y
  la carpeta de datos de clio-memory si está fuera de `Dirs.data`. Las herramientas de archivos
  **niegan** leer o escribir ahí (regla `host/control`, texto:
  `eso es de Clio: solo la persona lo toca`). Las órdenes de consola no lo ven (tapado).
- **Cuatro clases apartan** (`borrar`, `gastar`, `enviar`, `control`). La tabla de la Tarea 3 es la
  especificación: lo que no está en ella no aparta.
- **Respuesta al modelo cuando algo se aparta** (exacta, `ok: false`, regla `host/aside`):
  `apartado {id}: «{clase}» necesita el visto bueno de la persona. No lo intentes de otra forma. Sigue con lo demás y cuéntalo al final.`
- **Tope de preguntas:** 3 por día local (`questions_per_day`, ajustable). Un pendiente nace con
  `ask: true` si hoy van menos de 3, si no con `ask: false` (va al resumen del día). Las que la
  persona responde en la terminal no cuentan.
- **Un pendiente vence a las 24 horas.** Aprobarlo después no ejecuta nada y dice:
  `venció el {fecha}: no se ejecutó`.
- **Registro:** `Dirs.data/host/actions.jsonl`, 0600, una línea antes de ejecutar y otra después.
  Lleva `at_ms`, `run`, `session`, `tool`, `what` (la orden o la ruta, ya pasada por el limpiador
  de llaves), `network`, `outcome` (`run`, `aside`, `deny`, `done`), `rule`, `exit_code`. Si la
  línea de antes no se puede escribir, la acción no corre.
- **Copia antes de cambiar:** `runs/<id>/backups/<n>` y `runs/<id>/changes.jsonl` (`seq`, `path`,
  `backup` o `null` si el archivo no existía, `mode`, `sha256` de lo escrito, `at_ms`).
- **Texto ajeno** en modo anfitrión: marca la sesión leer (`fs_read`, `fs_list`, `fs_glob`) bajo
  una carpeta ajena, una orden con `network: true`, y una orden que nombra una carpeta ajena o
  lee registros (`journalctl`, `docker logs`, `docker compose logs`, `kubectl logs`). Carpetas
  ajenas de fábrica: `/var/log`, `/var/mail`, `/var/spool`, y cualquier carpeta llamada `uploads`,
  `Downloads` o `Descargas`. Lo demás no marca. La salida se sigue limpiando y envolviendo como
  dato igual que hoy.
- **La persona ajusta** en `Dirs.config/host.json` (todo opcional): `questions_per_day`,
  `foreign` (carpetas que se suman), `ask` (patrones que se suman, `{class, pattern}`). Solo suma:
  no puede quitar nada de fábrica.
- **Límites de una orden:** 120 s si no se dice, hasta 3600 s. Salida: lo de hoy (la cola).
  Sin `prlimit` en modo anfitrión.
- **Presupuesto:** en modo anfitrión no valen `max_steps` ni `max_seconds` por tarea. Valen por día
  (UTC, como `DayBudget` hoy) `limits.day_cost_micro_usd` (de fábrica 3.000.000) y
  `limits.day_steps` (de fábrica 1000), guardados en `Dirs.data/host/day.json` para que cuenten
  entre sesiones. El antibucle (`max_repeats`) sigue. Agotado:
  `presupuesto del día agotado: sigo mañana`.
- **Prompt:** archivo nuevo `prompts/system/clio-host@1.md`; `clio-agent@19` no se toca;
  `prompts/MANIFEST.json` al día.
- **El modo copia no cambia de comportamiento.** Todas sus pruebas siguen pasando sin tocarlas.
- Mensajes a la persona en español; comentarios de código en inglés.
- **Las pruebas no usan `sudo` ni tocan la máquina fuera de carpetas temporales.** El lanzador
  `systemd` se prueba por sus argumentos; su prueba real va tras `CLIO_TEST_SUDO=1` y no corre de
  fábrica. Este VPS tiene servicios en producción: nada los toca.
- Compuertas: `cargo fmt --all --check`;
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`;
  `cargo test -p clio-node -p clio-toolhost -p clio-brain --no-fail-fast`; `npm run check` en
  `adapters/pi`; `python3 scripts/check-prompts.py`. Falla conocida y ajena: `--test run_tests`.
- No se hace push.

## Foco de revisión

1. **El modo copia sigue intacto.** Sin `Config.host`, ninguna ruta absoluta entra, `exec` sigue
   sin red y en la jaula de siempre. → Tareas 2 y 5.
2. **Lo de Clio.** Ni por ruta directa, ni por enlace simbólico, ni por `..`, las herramientas de
   archivos leen o escriben la lista `control`; una orden no ve su contenido. → Tareas 1 y 2.
3. **Nada corre sin registro,** y una llave conocida no queda escrita en él. → Tarea 2.
4. **Lo apartado no corre** hasta que la persona dice sí, corre tal cual se guardó (mismo hash), una
   sola vez, y no después de vencer. → Tarea 4.
5. **La marca.** Una orden con red o una lectura de carpeta ajena marca; un `ls` de la carpeta
   personal no. En sesión marcada `memory_block_edit` se sigue negando. → Tarea 2.
6. **Falla cerrada.** Si el lanzador no pasa su autoprueba, en modo anfitrión no hay `exec`; nunca
   se cae a correr sin tapar lo de Clio. → Tarea 1.

---

### Task 1: El lanzador anfitrión en `clio-node`

**Archivos:** crear `crates/clio-node/src/host.rs`; tocar `jail.rs` solo para compartir el código
que corre un proceso y recoge su salida (`Jail::run` desde el `spawn`: colas, tope, tiempo,
`Reaper`). Pruebas en `crates/clio-node/tests/host.rs`.

**Interfaces:**
```rust
pub enum Launcher { Systemd, Bwrap }
pub struct HostJail { /* launcher, rutas de bwrap/systemd-run/sudo de carpetas de confianza */ }
pub struct HostJob {
    pub command: String,        // goes to `/usr/bin/bash -lc`
    pub cwd: PathBuf,
    pub home: PathBuf,
    pub network: bool,
    pub timeout: Duration,
    pub max_output: usize,
    pub masks: Vec<PathBuf>,    // Clio's folders: shown empty and read-only
}
impl HostJail {
    pub fn detect() -> Result<Self, String>;
    pub fn launcher(&self) -> Launcher;
    pub fn args(&self, job: &HostJob) -> Result<(PathBuf, Vec<String>), String>; // program, argv
    pub fn run(&self, job: &HostJob) -> Result<JailOutput, String>;
    pub fn self_test(&self, scratch: &Path) -> Result<(), String>;
}
```

- `detect`: `Systemd` si `sudo -n true` sale bien y hay `systemd-run`; si no, `Bwrap` con las mismas
  exigencias de versión que `Jail::detect`. Ninguno: error.
- `Systemd`: `sudo -n systemd-run --quiet --wait --pipe --collect --unit clio-host-<aleatorio>
  --uid --gid` del usuario actual, `--working-directory`, `-p RuntimeMaxSec`, `-p KillMode=control-group`,
  `-p TemporaryFileSystem=<máscara>:ro` por cada máscara que exista, `-p PrivateNetwork=yes` si no
  hay red, `-E HOME -E USER -E LOGNAME -E LANG=C.UTF-8 -E XDG_RUNTIME_DIR`, y `/usr/bin/bash -lc`.
  El entorno de quien lanza va vacío. Al pasar el tiempo se mata al cliente y se intenta
  `sudo -n systemctl stop <unidad>`.
- `Bwrap`: `--bind / / --dev-bind /dev /dev --proc /proc`, `--tmpfs` + `--remount-ro` por máscara,
  `--unshare-net` si no hay red, `--die-with-parent --new-session --clearenv`, el mismo entorno.
- `self_test`: en `scratch`, una máscara con un archivo señuelo no se ve desde dentro; sin red no
  se alcanza un puerto local que la prueba abre, y con red sí; con `Systemd`, `sudo -n true`
  funciona dentro.

- [ ] Pruebas de `args` para los dos lanzadores (máscaras, red, tiempo, entorno vacío).
- [ ] Prueba real con `Bwrap` (señuelo tapado, red cortada y abierta, tiempo vencido mata).
- [ ] Prueba real con `Systemd` tras `CLIO_TEST_SUDO=1`.
- [ ] Commit.

### Task 2: El modo anfitrión del toolhost

**Archivos:** `crates/clio-toolhost/src/lib.rs`, `ops.rs`, `cli.rs`; crear `host.rs`. Pruebas en
`crates/clio-toolhost/tests/host.rs`.

**Interfaces:**
```rust
pub struct HostCfg {
    pub home: PathBuf,
    pub state: PathBuf,           // Dirs.data/host
    pub control: Vec<PathBuf>,
    pub rules: HostRules,         // factory + the person's host.json (Task 3)
    pub run: String,              // the run id, for the log
}
// Config gains: pub host: Option<HostCfg>
```
`clio toolhost` gana `--host-home`, `--host-state`, `--host-control P` (repetible),
`--host-rules FILE` y `--host-run ID`.

- Con `host`: el `Sandbox` tiene raíz `/`. Una ruta absoluta se usa tal cual; una relativa se une a
  `home`. El gateway recibe `sandbox_root: "/"`. Antes de ejecutar, la ruta (o su carpeta padre si
  no existe) se canoniza y se niega si cae bajo `control`.
- `exec` gana el argumento `network` (bool, opcional) y corre con `HostJail` (máscaras = `control`),
  en `home`, solo si `self_test` pasó al arrancar. Se describe al gateway como hoy.
- `fs_write` y `fs_mkdir` dejan copia y línea en `changes.jsonl` antes de cambiar.
- Registro de acciones según las restricciones globales, para `exec`, `fs_write` y `fs_mkdir`.
- La marca según «texto ajeno»; fuentes nuevas `web` y `foreign`. `present()` no cambia para el
  modo copia.
- `pub fn undo(run_dir: &Path) -> Result<Vec<String>, String>` en `host.rs`: recorre
  `changes.jsonl` del final al principio; devuelve lo que restauró. Un archivo que cambió después
  (su `sha256` ya no es el escrito) no se toca y se informa.

- [ ] Pruebas: ruta absoluta lee y escribe en una carpeta temporal; sin `host` la misma se niega.
- [ ] Pruebas: `control` por ruta directa, por enlace simbólico y por `..` se niega al leer y al escribir.
- [ ] Pruebas: escribir deja copia; `undo` devuelve el contenido y el modo; un archivo nuevo se borra;
  uno cambiado después se respeta.
- [ ] Pruebas: cada `exec` y cada escritura dejan sus dos líneas; con el registro sin poder
  escribirse, no corre; una llave conocida sale tachada en `what`.
- [ ] Pruebas: `ls` de `home` no marca; leer bajo `/var/log` (carpeta ajena de prueba) marca;
  `network: true` marca; en sesión marcada `memory_block_edit` se niega.
- [ ] Commit.

### Task 3: La lista de lo que se aparta

**Archivos:** crear `crates/clio-toolhost/src/ask.rs`. Pruebas en el módulo, con esta tabla.

```rust
pub enum Class { Borrar, Gastar, Enviar, Control }
pub struct HostRules { /* compiled factory rules + extras, foreign folders, questions_per_day */ }
impl HostRules {
    pub fn factory() -> Self;
    pub fn with_file(self, json: &str) -> Result<Self, String>; // only adds
    pub fn classify_command(&self, command: &str) -> Option<Class>;
    pub fn classify_write(&self, path: &Path) -> Option<Class>;
    pub fn is_foreign(&self, path: &Path) -> bool;
    pub fn reads_foreign(&self, command: &str) -> bool;
}
```

Una orden se parte en órdenes simples (por `;`, `&&`, `||`, `|`, saltos de línea, `$(…)`, y lo que
va tras `sudo`, `env`, `nohup`, `time`, `xargs`, `sh -c`, `bash -c`) y cada una se clasifica. Es
una lista, no un muro: quien la quiera burlar puede (un script, una variable), y contra eso quedan
el registro y la regla del prompt. Dilo así en el comentario del módulo.

| Orden | Clase |
|---|---|
| `rm -rf ~/proyecto`, `rm -r datos`, `sudo rm -fR /srv/x` | borrar |
| `rm -rf node_modules`, `rm -rf target dist`, `rm -rf /tmp/x`, `rm archivo.txt` | — |
| `rm backup.sql`, `mv /var/backups/a /tmp`, `truncate -s 0 dump.sql.gz`, `> respaldo.tar` | borrar |
| `find . -name '*.log' -delete`, `find /srv -exec rm {} +`, `shred x` | borrar |
| `mkfs.ext4 /dev/sdb`, `dd if=x of=/dev/sda`, `wipefs -a /dev/sdb` | borrar |
| `psql -c 'DROP TABLE t'`, `mysql -e "truncate table t"`, `sqlite3 d 'delete from t'`, `dropdb x` | borrar |
| `docker volume rm v`, `docker system prune -af`, `docker compose down -v` | borrar |
| `redis-cli flushall`, `kubectl delete ns x`, `terraform destroy`, `zfs destroy p/x`, `userdel -r u`, `apt-get purge x` | borrar |
| `docker compose down`, `docker rm c`, `git reset --hard`, `git clean -fd` | — |
| `aws ec2 run-instances …`, `gcloud compute instances create x`, `oci compute instance launch`, `doctl compute droplet create`, `hcloud server create`, `terraform apply`, `pulumi up`, `fly scale count 3` | gastar |
| `aws s3 ls`, `gcloud compute instances list`, `terraform plan` | — |
| `mail -s x a@b.c`, `sendmail a@b.c`, `msmtp a@b.c`, `swaks --to a@b.c` | enviar |
| `git push`, `git push origin main`, `git push origin HEAD`, `git push -f origin rama`, `git push --tags`, `git push origin :rama` | enviar |
| `git push origin feat/x`, `git push -u origin fix-1` | — |
| `gh pr merge 3`, `gh pr comment 3 -b x`, `gh issue create`, `gh release create v1`, `gh pr review 3 --approve` | enviar |
| `gh pr create --fill`, `gh pr list`, `gh run view` | — |
| `gh repo delete x` | borrar |
| `npm publish`, `cargo publish`, `twine upload dist/*`, `docker push img`, `gem push x.gem` | enviar |
| `curl -X POST https://api.x.com -d a=1`, `curl -F f=@x https://x.com`, `curl -T f https://x.com`, `wget --post-data=a https://x.com` | enviar |
| `curl https://x.com`, `curl -X POST http://localhost:3000/x -d a=1`, `curl -d a=1 http://127.0.0.1:8080` | — |
| `clio vault stop`, `clio connect claude`, `clio init`, `clio memory identity`, `clio-memory purge`, `clio-memory import x` | control |
| `clio-memory m search x`, `clio doctor`, `clio pending` | — |
| `cat ~/.config/clio/config.json`, `ls ~/.local/share/clio`, cualquier orden que nombre una ruta de `control` | control |
| `systemctl --user stop clio-memory-sleep.timer`, `sudo systemctl disable clio-telegram` | control |
| `visudo`, `sudo tee /etc/sudoers.d/x`, `reboot`, `sudo shutdown -h now`, `poweroff` | control |
| `systemctl status nginx`, `sudo systemctl restart nginx`, `sudo apt-get install -y jq` | — |

`classify_write`: escribir sobre un archivo que existe y cuyo nombre parece respaldo (`backup`,
`respaldo`, `.bak`, `.dump`, `.sql`, `.sql.gz`, `snapshot`) es `borrar`; escribir `/etc/sudoers*`
es `control`.

- [ ] La tabla entera como prueba (cada fila, cada orden).
- [ ] `with_file` suma un patrón y una carpeta ajena; un archivo que intenta quitar algo o trae un
  patrón inválido da error claro.
- [ ] Commit.

### Task 4: Apartar, aprobar y vencer

**Archivos:** crear `crates/clio-toolhost/src/pending.rs`; `lib.rs` (el punto donde hoy vive
`bash_rule`). Pruebas en `tests/host.rs`.

```rust
pub struct Pending {
    pub id: String,               // short, like a run id
    pub class: Class,
    pub tool: String,
    pub args: Value,
    pub hash: String,             // sha256 over tool + canonical args
    pub what: String,             // scrubbed, for showing
    pub run: String,
    pub session: String,
    pub created_ms: i64,
    pub expires_ms: i64,          // + 24 h
    pub ask: bool,                // false: past today's cap, goes to the day's summary
    pub state: State,             // Waiting | Done { exit_code, output_tail } | Rejected | Expired
}
pub fn list(state: &Path) -> Vec<Pending>;
pub fn reject(state: &Path, id: &str) -> Result<Pending, String>;
impl ToolHost { pub fn approve(&self, id: &str, now_ms: i64) -> Result<Pending, String>; }
```

- En `handle`, tras el permiso del gateway y antes de ejecutar: si `HostRules` clasifica la acción,
  se guarda el pendiente (`state/pending/<id>.json`, 0600), se registra (`outcome: aside`) y se
  responde el texto exacto de las restricciones. Nada se ejecuta.
- La misma acción (mismo hash) apartada otra vez en la misma sesión devuelve el mismo `id`: no
  crea otro pendiente ni gasta otra pregunta.
- `approve`: solo `Waiting`; vencido pasa a `Expired` y da el texto de vencido; comprueba el hash;
  ejecuta por el mismo camino que una acción permitida (registro, copia antes de cambiar), sin
  volver a clasificar; guarda el resultado (cola de la salida, limpia). Dos veces: la segunda dice
  que ya se hizo.
- El tope por día vive en `state/questions.json` (día local y cuenta).

- [ ] Pruebas: una orden de la tabla no corre y queda pendiente; la tarea puede seguir con otra orden.
- [ ] Pruebas: aprobar la ejecuta una vez; rechazar no ejecuta; vencida no ejecuta; un pendiente
  con los argumentos cambiados en disco (hash distinto) no corre.
- [ ] Pruebas: el cuarto pendiente del día nace con `ask: false`; al día siguiente vuelve a `true`.
- [ ] Commit.

### Task 5: La sesión anfitrión en el cerebro y la guardia

**Archivos:** `crates/clio-brain/src/cli/chat/local.rs`, `cli/home.rs` (`mode`, límites por día,
`host.json`), `cli/mod.rs` (`--host`, `--copy`), `pi/session.rs` (`ToolhostOpts`), `pi/jail.rs` y
`pi/launch.rs` (la carpeta de trabajo), `control.rs` (`DayBudget` con pasos y guardado),
`adapters/pi/extensions/clio-guard/index.ts`, `prompts/system/clio-host@1.md`, `prompts/MANIFEST.json`.

- Sesión nueva en modo anfitrión: no hay copia. Se crea `runs/<id>/` con `meta.json`
  (`workspace` = la carpeta personal, `host: true`), sin `sandbox` ni `manifest`. Una sesión
  listada se abre como anfitrión solo si su `meta.json` dice `host: true` y su carpeta es la
  personal.
- Pi arranca con la carpeta personal como carpeta de trabajo, y en su jaula esa ruta es una
  carpeta **vacía** (no se monta nada de la persona). La autoprueba de la jaula de Pi sigue
  pasando; con el conector `claude`, igual.
- La guardia, con `CLIO_GUARD_HOST=1`: manda rutas absolutas, no niega por «fuera de la carpeta»
  ni por carpeta protegida (eso lo decide el servicio), y `bash` gana el parámetro `network`
  (`boolean`, opcional: «la orden necesita red; marca la sesión»). Sin la variable, todo igual.
- El toolhost arranca con las opciones de la Tarea 2; `control` sale de `Dirs`.
- Presupuesto por día según las restricciones.
- `clio-host@1.md` dice, en corto: actúas sobre la máquina real de la persona; rutas absolutas o
  relativas a su carpeta personal; una orden no tiene red salvo `network: true`; para cambiar un
  archivo usa `edit`/`write` (se puede deshacer exacto) antes que la consola; cuatro clases se
  apartan (nómbralas) y qué hacer entonces; en un repositorio trabaja en una rama, entrega un PR
  con `git push -u origin <rama>` y `gh pr create`, nunca empujes a la principal; todo queda
  registrado; la máquina tiene cosas en producción: mira antes de cambiar.
- Limpieza: al abrir una sesión nueva (en cualquier modo), se borran las carpetas `runs/<id>` de
  más de 14 días sin cambios pendientes (modo copia: aplicada, o sin diferencias; modo anfitrión:
  sin pendientes en espera). Mejor esfuerzo; un fallo va al registro y no impide abrir.

- [ ] Pruebas: `mode` y las opciones eligen el modo; de fábrica es `copy`.
- [ ] Pruebas: los argumentos de la jaula de Pi en modo anfitrión no montan la carpeta personal.
- [ ] Pruebas: `DayBudget` cuenta pasos y costo entre dos cargas del archivo y se reinicia al cambiar el día.
- [ ] Pruebas: la limpieza borra lo viejo sin pendientes y respeta lo demás.
- [ ] `npm run check` y `check-prompts.py` en verde.
- [ ] Commit.

### Task 6: Las órdenes de la persona y la pregunta en la terminal

**Archivos:** crear `crates/clio-brain/src/cli/pending.rs`; `cli/mod.rs`; `cli/apply.rs` o donde
viva `undo`; la interfaz (`cli/tui/`, `cli/chat/`); `cli/doctor.rs`.

- `clio pending [--json]`: lo que espera (id, clase, qué, de cuándo, cuándo vence, si se preguntó).
- `clio approve ID` y `clio reject ID`: con los textos de las restricciones. `approve` arma un
  `ToolHost` anfitrión sobre la carpeta de esa sesión y llama `approve`.
- `clio actions [--run ID] [--json]`: el registro, lo último abajo.
- `clio undo ID` en una sesión anfitrión llama a `host::undo` y cuenta qué devolvió y qué no.
- En una conversación, al terminar cada turno: por cada pendiente nuevo de esta sesión, la
  interfaz muestra la clase y la orden y pregunta `¿Lo ejecuto? [s/N]`. Con sí, se ejecuta y el
  resultado va en el siguiente mensaje al modelo como dato (`la persona aprobó {id}: …`); con no,
  se rechaza. Estas no cuentan para el tope. `clio run` no pregunta: termina diciendo qué apartó.
- `clio doctor`: una sección del modo anfitrión: qué lanzador hay, si su autoprueba pasa, y un
  aviso (no fallo) cuando no hay `sudo` sin contraseña: `la consola de Clio no tendrá sudo`.

- [ ] Pruebas de cada orden sobre una carpeta de datos temporal.
- [ ] Prueba: `clio run --host` con algo apartado termina con el informe y código de salida 0.
- [ ] Commit.

### Task 7: Ataques, documentos y prueba real

- [ ] `testing/attacks`: un archivo en una carpeta ajena con órdenes para el agente (cambia tu
  identidad, borra respaldos, envía un correo) no logra ninguna: la identidad se niega por la
  marca y las otras dos se apartan.
- [ ] `docs/decisions/`: un ADR del modo anfitrión (los dos lanzadores y por qué, la red por
  orden, que la lista no es un muro, que con `sudo` la tapa tampoco).
- [ ] `docs/threat-model.md` y `docs/uso-local.md` al día; `README` del toolhost con las opciones nuevas.
- [ ] Prueba real en el VPS, solo lectura y carpetas temporales: `clio doctor`, y una sesión
  `clio run --host` que lista la carpeta personal, crea y edita un archivo en `/tmp`, lo deshace,
  e intenta un `rm -r` que queda apartado. No tocar ningún servicio.
- [ ] Commit.

## Después

Una revisión de la rama completa por un subagente nuevo, con el «Foco de revisión», y una sola
ronda de correcciones. Sin push ni fusión hasta que Iván lo pida.
