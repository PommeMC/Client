//! Runs pomme's in-game test scripts (`tests/ingame/**/*.toml`) against local
//! vanilla servers: downloads and verifies each version's server jar once,
//! starts it offline with the test player opped, then runs a harness client
//! per script.
//!
//! `ingame-test [--jobs N] [--version V]... [--versions all] [scripts...]`
//! runs the named scripts or folders (all when none, e.g. `entities` or
//! `entities/data`) on N workers per version, each with its own server and
//! world, against the native version unless `--version` or `--versions all`
//! picks others; `ingame-test --server-only` starts a server for live socket
//! sessions. `--capture` also records each script's clientbound frames into
//! its `wire-fixtures/` corpus entry.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitCode, Stdio};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use md5::Digest as _;
use pomme_protocol::version::{self, ProtocolVersion};

const MANIFEST_URL: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const PLAYER: &str = "Tester";
const HOST: &str = "127.0.0.1";
const SERVER_START_TIMEOUT: Duration = Duration::from_secs(180);
const SERVER_STOP_TIMEOUT: Duration = Duration::from_secs(30);
const PLAYER_LEAVE_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_JOBS: usize = 6;

const OUT_DIR: &str = "target/ingame";
const SCRIPT_DIR: &str = "tests/ingame";
const FIXTURE_DIR: &str = "pomme-client/src/net/wire-fixtures";
/// Game packets kept per packet id in a fixture, so repeats (chunks above all)
/// don't bloat it.
const FIXTURE_FRAMES_PER_PACKET: usize = 3;

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let versions = match take_versions(&mut args) {
        Ok(versions) => versions,
        Err(e) => {
            eprintln!("ingame-test: {e}");
            return ExitCode::from(2);
        }
    };
    // Just the verified server jar, for tools that run vanilla's code.
    if take_flag(&mut args, "--fetch-server") {
        return match server_jar(&versions[0]) {
            Ok(server) => {
                println!("{}", server.jar.display());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("ingame-test: {e}");
                ExitCode::from(2)
            }
        };
    }
    let server_only = take_flag(&mut args, "--server-only");
    let capture = take_flag(&mut args, "--capture");
    let jobs = match take_value(&mut args, "--jobs").map(|n| n.parse::<usize>()) {
        None => DEFAULT_JOBS,
        Some(Ok(n)) if n > 0 => n,
        Some(_) => {
            eprintln!("ingame-test: --jobs takes a positive number");
            return ExitCode::from(2);
        }
    };
    match run(&versions, server_only, jobs, capture, &args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("ingame-test: {e}");
            ExitCode::from(2)
        }
    }
}

fn take_flag(args: &mut Vec<String>, flag: &str) -> bool {
    let before = args.len();
    args.retain(|a| a != flag);
    args.len() != before
}

/// The value after `flag` (empty when missing), removing both.
fn take_value(args: &mut Vec<String>, flag: &str) -> Option<String> {
    let i = args.iter().position(|a| a == flag)?;
    args.remove(i);
    Some(if i < args.len() {
        args.remove(i)
    } else {
        String::new()
    })
}

/// The versions `--version` (repeatable) and `--versions all` pick, else the
/// native one.
fn take_versions(args: &mut Vec<String>) -> Result<Vec<ProtocolVersion>, String> {
    let mut versions = match take_value(args, "--versions").as_deref() {
        None => Vec::new(),
        Some("all") => version::tested_versions(),
        Some(other) => return Err(format!("--versions takes `all`, not {other:?}")),
    };
    while let Some(name) = take_value(args, "--version") {
        let found = ProtocolVersion::from_name(&name)
            .filter(|v| version::joinable(v.protocol))
            .ok_or_else(|| format!("{name:?} isn't a joinable version"))?;
        if !versions.contains(&found) {
            versions.push(found);
        }
    }
    if versions.is_empty() {
        versions.push(version::NATIVE);
    }
    Ok(versions)
}

/// Whether every script passed on every version.
fn run(
    versions: &[ProtocolVersion],
    server_only: bool,
    jobs: usize,
    capture: bool,
    names: &[String],
) -> Result<bool, String> {
    let scripts = scripts(Path::new(SCRIPT_DIR), names)?;

    if server_only {
        let version = versions[0];
        let server = server_jar(&version)?;
        let java = java(server.java_major)?;
        let mut server = Server::start(&java, &server.jar, &server.dir)?;
        println!(
            "Vanilla {} server ready on {HOST}:{}",
            version.name, server.port
        );
        println!(
            "Join with: just client-dev --features test-harness -- {} --harness-port <port>",
            join_args(&version, server.port).join(" ")
        );
        server.wait();
        return Ok(true);
    }

    build_client()?;
    let mut failed = Vec::new();
    let mut total = 0;
    for version in versions {
        let (runnable, skipped): (Vec<_>, Vec<_>) = scripts
            .iter()
            .map(|script| (script, script_runs_on(script, version)))
            .partition(|(_, runs)| matches!(runs, Ok(true)));
        for (script, runs) in skipped {
            runs?;
            println!(
                "SKIP {} {}",
                version.name,
                script_name(Path::new(SCRIPT_DIR), script)
            );
        }
        let runnable: Vec<&PathBuf> = runnable.into_iter().map(|(script, _)| script).collect();
        total += runnable.len();
        failed.extend(run_version(version, &runnable, jobs, capture)?);
    }
    failed.sort();
    println!("\n{} of {total} scripts passed", total - failed.len());
    for script in &failed {
        println!("  FAILED {script}");
    }
    // A capture records the server's frames whether or not the script passed.
    Ok(failed.is_empty() || capture)
}

/// Runs `scripts` against one version's servers; the failed ones, as
/// `<version> <script>`.
fn run_version(
    version: &ProtocolVersion,
    scripts: &[&PathBuf],
    jobs: usize,
    capture: bool,
) -> Result<Vec<String>, String> {
    let Some((first, rest)) = scripts.split_first() else {
        return Ok(Vec::new());
    };
    let jar = server_jar(version)?;
    let java = java(jar.java_major)?;
    let worker_dir = |n: usize| {
        Path::new(OUT_DIR)
            .join("workers")
            .join(version.name)
            .join(n.to_string())
    };
    let failed = Mutex::new(Vec::new());
    let run = |script: &Path, server: &mut Server, game_dir: &Path| -> Result<(), String> {
        if !run_script(script, version, server.port, game_dir, capture)? {
            let name = script_name(Path::new(SCRIPT_DIR), script);
            failed
                .lock()
                .unwrap()
                .push(format!("{} {name}", version.name));
        }
        server.reset_player()
    };

    // The first client installs the version's assets into the shared
    // folders, so it runs alone rather than racing the others' installs.
    let dir = worker_dir(0);
    let mut server = Server::start(&java, &jar.jar, &dir)?;
    run(first, &mut server, &dir.join("client"))?;

    let queue = Mutex::new(rest.iter());
    let mut first_server = Some(server);
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..jobs.min(rest.len()))
            .map(|n| {
                let (queue, run, java, jar) = (&queue, &run, &java, &jar.jar);
                // Worker 0 keeps the warm-up's server.
                let server = first_server.take();
                scope.spawn(move || -> Result<(), String> {
                    let dir = worker_dir(n);
                    let mut server = match server {
                        Some(server) => server,
                        None => Server::start(java, jar, &dir)?,
                    };
                    let game_dir = dir.join("client");
                    loop {
                        let next = queue.lock().unwrap().next();
                        let Some(script) = next else {
                            return Ok(());
                        };
                        run(script, &mut server, &game_dir)?;
                    }
                })
            })
            .collect();
        workers
            .into_iter()
            .try_for_each(|worker| worker.join().expect("worker panicked"))
    })?;
    Ok(failed.into_inner().unwrap())
}

/// The named scripts and folders under `root`, or every script in it.
fn scripts(root: &Path, names: &[String]) -> Result<Vec<PathBuf>, String> {
    if names.is_empty() {
        return scripts_under(root);
    }
    let mut all = Vec::new();
    for name in names {
        let path = root.join(name);
        let script = path.with_extension("toml");
        if script.is_file() {
            all.push(script);
        } else if path.is_dir() {
            all.extend(scripts_under(&path)?);
        } else {
            return Err(format!("no script or folder {}", path.display()));
        }
    }
    Ok(all)
}

/// Every script in `dir` and its subfolders, sorted.
fn scripts_under(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut all = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_dir() {
            all.extend(scripts_under(&path)?);
        } else if path.extension().is_some_and(|ext| ext == "toml") {
            all.push(path);
        }
    }
    all.sort();
    Ok(all)
}

/// A script's name: its path under `root` without `.toml`, e.g.
/// `entities/data`, which also names its output folder.
fn script_name(root: &Path, script: &Path) -> String {
    let relative = script.strip_prefix(root).unwrap_or(script);
    relative
        .with_extension("")
        .to_string_lossy()
        .replace('\\', "/")
}

/// Whether `script` runs on `version`: its optional top-level `min_version`
/// and `max_version` bound the versions it's valid on.
fn script_runs_on(script: &Path, version: &ProtocolVersion) -> Result<bool, String> {
    let text = std::fs::read_to_string(script).map_err(|e| format!("{}: {e}", script.display()))?;
    let table: toml::Table = text
        .parse()
        .map_err(|e| format!("{}: {e}", script.display()))?;
    in_range(&table, version).map_err(|e| format!("{}: {e}", script.display()))
}

/// Whether `version` lies within `table`'s `min_version`/`max_version`.
fn in_range(table: &toml::Table, version: &ProtocolVersion) -> Result<bool, String> {
    // VERSIONS is newest first, so an older version has a larger index.
    let age = |name: &str| version::VERSIONS.iter().position(|v| v.name == name);
    let bound = |key: &str| -> Result<Option<usize>, String> {
        match table.get(key) {
            None => Ok(None),
            Some(toml::Value::String(name)) => age(name)
                .map(Some)
                .ok_or_else(|| format!("{key} {name:?} isn't a listed version")),
            Some(_) => Err(format!("{key} must be a version name")),
        }
    };
    let age_of = age(version.name).expect("a listed version");
    let newer_than_min = bound("min_version")?.is_none_or(|min| age_of <= min);
    let older_than_max = bound("max_version")?.is_none_or(|max| age_of >= max);
    Ok(newer_than_min && older_than_max)
}

/// A version's verified server jar, its dir and the Java major it needs.
struct ServerJar {
    dir: PathBuf,
    jar: PathBuf,
    java_major: u32,
}

/// The verified server jar for `version`, downloaded on first use. The
/// version's metadata (download url, sha1, Java major) is looked up in
/// Mojang's version manifest once and cached next to the jar, and looked up
/// again if the cache can't be read.
fn server_jar(version: &ProtocolVersion) -> Result<ServerJar, String> {
    let dir = Path::new(OUT_DIR).join("servers").join(version.name);
    let meta_path = dir.join("version.json");
    let cached = std::fs::read(&meta_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let meta: serde_json::Value = match cached {
        Some(meta) => meta,
        None => {
            let meta = version_meta(version.name)?;
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            std::fs::write(&meta_path, meta.to_string()).map_err(|e| e.to_string())?;
            meta
        }
    };
    let server = &meta["downloads"]["server"];
    let (Some(url), Some(sha1_hex)) = (server["url"].as_str(), server["sha1"].as_str()) else {
        return Err(format!("{} has no server download", version.name));
    };
    let java_major = meta["javaVersion"]["majorVersion"]
        .as_u64()
        .ok_or_else(|| format!("{} names no Java version", version.name))?
        as u32;
    let jar = dir.join("server.jar");
    if !std::fs::read(&jar).is_ok_and(|bytes| sha1(&bytes) == sha1_hex) {
        println!("Downloading the vanilla {} server...", version.name);
        let bytes = fetch(url)?;
        if sha1(&bytes) != sha1_hex {
            return Err(format!(
                "the {} server jar doesn't match its sha1",
                version.name
            ));
        }
        std::fs::write(&jar, &bytes).map_err(|e| e.to_string())?;
    }
    Ok(ServerJar {
        dir,
        jar,
        java_major,
    })
}

/// `name`'s version JSON, through Mojang's version manifest.
fn version_meta(name: &str) -> Result<serde_json::Value, String> {
    let manifest: serde_json::Value =
        serde_json::from_slice(&fetch(MANIFEST_URL)?).map_err(|e| e.to_string())?;
    let url = manifest["versions"]
        .as_array()
        .and_then(|all| all.iter().find(|v| v["id"] == name))
        .and_then(|v| v["url"].as_str())
        .ok_or_else(|| format!("{name} isn't in Mojang's version manifest"))?;
    serde_json::from_slice(&fetch(url)?).map_err(|e| e.to_string())
}

fn fetch(url: &str) -> Result<Vec<u8>, String> {
    reqwest::blocking::get(url)
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.bytes())
        .map(|bytes| bytes.to_vec())
        .map_err(|e| format!("downloading {url}: {e}"))
}

fn sha1(bytes: &[u8]) -> String {
    sha1_smol::Sha1::from(bytes).digest().to_string()
}

/// A java for a server needing Java `major`: `$JAVA_HOME_<major>`, else
/// `$JAVA_HOME`, else `java` on PATH, checked to be at least that major.
fn java(major: u32) -> Result<PathBuf, String> {
    let java = std::env::var_os(format!("JAVA_HOME_{major}"))
        .or_else(|| std::env::var_os("JAVA_HOME"))
        .map(|home| Path::new(&home).join("bin").join("java"))
        .unwrap_or_else(|| "java".into());
    let output = Command::new(&java)
        .arg("-version")
        .output()
        .map_err(|e| format!("running {}: {e} (install Java {major})", java.display()))?;
    // `openjdk version "25.0.2" ...` on stderr.
    let banner = String::from_utf8_lossy(&output.stderr);
    let found = banner
        .split('"')
        .nth(1)
        .and_then(|v| v.split('.').next()?.parse::<u32>().ok());
    match found {
        Some(found) if found >= major => Ok(java),
        _ => Err(format!(
            "this server needs Java {major}+ (JAVA_HOME_{major} or JAVA_HOME), found: {}",
            banner.lines().next().unwrap_or("nothing")
        )),
    }
}

fn free_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind((HOST, 0)).map_err(|e| e.to_string())?;
    Ok(listener.local_addr().map_err(|e| e.to_string())?.port())
}

/// A fresh flat world, offline, with the test player opped.
fn prepare_server(dir: &Path, port: u16) -> Result<(), String> {
    let world = dir.join("world");
    if world.exists() {
        std::fs::remove_dir_all(&world).map_err(|e| format!("{}: {e}", world.display()))?;
    }
    let properties = format!(
        "online-mode=false\n\
         enforce-secure-profile=false\n\
         server-ip={HOST}\n\
         server-port={port}\n\
         level-type=minecraft\\:flat\n\
         generate-structures=false\n\
         spawn-protection=0\n\
         view-distance=4\n\
         simulation-distance=4\n\
         motd=pomme in-game tests\n"
    );
    let uuid = offline_uuid(PLAYER);
    let ops = serde_json::json!([{
        "uuid": uuid.to_string(),
        "name": PLAYER,
        "level": 4,
        "bypassesPlayerLimit": false,
    }]);
    let write = |name: &str, contents: String| {
        std::fs::write(dir.join(name), contents).map_err(|e| format!("{name}: {e}"))
    };
    write("eula.txt", "eula=true\n".into())?;
    write("server.properties", properties)?;
    write("ops.json", ops.to_string())
}

/// Vanilla `UUIDUtil.createOfflinePlayerUUID`, as the client derives it.
fn offline_uuid(name: &str) -> uuid::Uuid {
    let digest = md5::Md5::digest(format!("OfflinePlayer:{name}").as_bytes());
    uuid::Builder::from_md5_bytes(digest.into()).into_uuid()
}

fn build_client() -> Result<(), String> {
    let status = Command::new(env!("CARGO"))
        .args(["build", "-p", "pomme-client", "--features", "test-harness"])
        .status()
        .map_err(|e| e.to_string())?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| "building the client failed".into())
}

/// Runs one script in a harness client joining `version`, logging to
/// `client.log` in its output dir; whether it passed. With `capture`, the
/// frames the server sent become the script's fixture for `version`.
fn run_script(
    script: &Path,
    version: &ProtocolVersion,
    port: u16,
    game_dir: &Path,
    capture: bool,
) -> Result<bool, String> {
    let name = script_name(Path::new(SCRIPT_DIR), script);
    let out = Path::new(OUT_DIR).join(version.name).join(&name);
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let log_path = out.join("client.log");
    let log = std::fs::File::create(&log_path).map_err(|e| e.to_string())?;
    let client =
        Path::new("target/debug").join(format!("pomme-client{}", std::env::consts::EXE_SUFFIX));
    let frames = out.join("frames.bin");
    let started = Instant::now();
    let mut command = Command::new(&client);
    command
        .args(join_args(version, port))
        .arg("--game-dir")
        .arg(game_dir)
        .arg("--harness-script")
        .arg(script)
        .arg("--harness-out")
        .arg(&out);
    if capture {
        command.arg("--capture-frames").arg(&frames);
    }
    let status = command
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log)
        .status()
        .map_err(|e| format!("starting {}: {e}", client.display()))?;
    let secs = started.elapsed().as_secs_f32();
    let passed = status.success();
    let v = version.name;
    if passed {
        println!("PASS {v} {name} ({secs:.1}s)");
    } else {
        println!("FAIL {v} {name} ({secs:.1}s), see {}", log_path.display());
    }
    if capture {
        write_fixture(&frames, version, &name)?;
    }
    Ok(passed)
}

/// Trims a script's captured frames to every login and config frame plus
/// the first few of each game packet, and writes them deflated as its
/// fixture for `version`.
fn write_fixture(frames: &Path, version: &ProtocolVersion, script: &str) -> Result<(), String> {
    use pomme_protocol::{Phase, capture};

    let bytes = std::fs::read(frames).map_err(|e| format!("{}: {e}", frames.display()))?;
    let records =
        capture::records(&bytes).ok_or_else(|| format!("{} is cut short", frames.display()))?;
    let mut kept = Vec::new();
    let mut seen = std::collections::HashMap::<u32, usize>::new();
    for (phase, frame) in records {
        if phase == Phase::Game {
            let id = pomme_protocol::codec::Reader::new(frame)
                .varint()
                .map_err(|e| format!("a frame in {}: {e}", frames.display()))?;
            let count = seen.entry(id).or_default();
            *count += 1;
            if *count > FIXTURE_FRAMES_PER_PACKET {
                continue;
            }
        }
        capture::write_record(&mut kept, phase, frame);
    }
    let path = Path::new(FIXTURE_DIR)
        .join(version.name)
        .join(script)
        .with_extension("bin");
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(&kept).map_err(|e| e.to_string())?;
    let deflated = encoder.finish().map_err(|e| e.to_string())?;
    std::fs::write(&path, &deflated).map_err(|e| format!("{}: {e}", path.display()))?;
    println!("  wrote {} ({} KB)", path.display(), deflated.len() / 1024);
    Ok(())
}

/// The client flags that join the test server as the test player.
fn join_args(version: &ProtocolVersion, port: u16) -> Vec<String> {
    [
        "--version",
        version.name,
        "--username",
        PLAYER,
        "--quick-access-multiplayer",
        &format!("{HOST}:{port}"),
    ]
    .map(str::to_owned)
    .into()
}

/// The vanilla server process; stopped when dropped.
struct Server {
    child: Child,
    stdin: ChildStdin,
    /// Console lines not yet waited past.
    lines: mpsc::Receiver<String>,
    dir: PathBuf,
    port: u16,
}

impl Server {
    /// Starts the server in `dir` on a fresh world and a free port.
    fn start(java: &Path, jar: &Path, dir: &Path) -> Result<Self, String> {
        let port = free_port()?;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        prepare_server(dir, port)?;
        let jar = std::path::absolute(jar).map_err(|e| e.to_string())?;
        let mut child = Command::new(java)
            .args(["-Xmx2G", "-jar"])
            .arg(&jar)
            .arg("nogui")
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("starting the server: {e}"))?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let log = dir.join("console.log");
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            let mut log = std::fs::File::create(log).ok();
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(log) = &mut log {
                    let _ = writeln!(log, "{line}");
                }
                let _ = tx.send(line);
            }
        });
        let mut server = Self {
            child,
            stdin,
            lines,
            dir: dir.to_owned(),
            port,
        };
        server.wait_for("Done (", SERVER_START_TIMEOUT)?;
        // Spawn chunks unload once the player leaves, taking their entities
        // out of the reset's reach; keep the area around spawn loaded.
        server.command("forceload add -48 -48 47 47");
        // Mobs spawning at night attack and push the test player mid-script.
        server.gamerule(["spawn_mobs", "doMobSpawning"], false);
        server.reset_world();
        Ok(server)
    }

    /// Waits for a console line containing `needle`.
    fn wait_for(&mut self, needle: &str, timeout: Duration) -> Result<(), String> {
        let started = Instant::now();
        while started.elapsed() < timeout {
            match self.lines.recv_timeout(Duration::from_millis(250)) {
                Ok(line) if line.contains(needle) => return Ok(()),
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                return Err(format!(
                    "the server exited ({status}); see {}",
                    self.dir.join("console.log").display()
                ));
            }
        }
        Err(format!("the server never logged {needle:?}"))
    }

    fn command(&mut self, command: &str) {
        let _ = writeln!(self.stdin, "{command}");
    }

    /// Sets a game rule by its 26.x and older names; each version rejects
    /// the name it lacks.
    fn gamerule(&mut self, names: [&str; 2], value: bool) {
        for name in names {
            self.command(&format!("gamerule {name} {value}"));
        }
    }

    /// Clears what a script left behind. Once the player has left and the
    /// world is saved, their data is deleted so the next join starts fresh.
    fn reset_player(&mut self) -> Result<(), String> {
        // Absent when the client never joined.
        let _ = self.wait_for(&format!("{PLAYER} left the game"), PLAYER_LEAVE_TIMEOUT);
        self.command("save-all flush");
        self.wait_for("Saved the game", SERVER_STOP_TIMEOUT)?;
        let uuid = offline_uuid(PLAYER);
        let players = self.dir.join("world/players");
        for file in [
            format!("data/{uuid}.dat"),
            format!("data/{uuid}.dat_old"),
            format!("advancements/{uuid}.json"),
            format!("stats/{uuid}.json"),
        ] {
            let _ = std::fs::remove_file(players.join(file));
        }
        // Twice: the first kill makes mobs drop loot and orbs, which the next
        // player would otherwise pick up.
        for _ in 0..2 {
            self.command("kill @e[type=!player]");
        }
        self.reset_world();
        Ok(())
    }

    /// Noon, clear and ticking, so a script that doesn't set the time or
    /// weather sees the same sky whether it runs first or after others.
    fn reset_world(&mut self) {
        self.gamerule(["advance_time", "doDaylightCycle"], true);
        self.command("time set 6000");
        self.command("weather clear");
    }

    /// Blocks until the server exits (Ctrl+C or `stop`).
    fn wait(&mut self) {
        let _ = self.child.wait();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.command("stop");
        let started = Instant::now();
        while started.elapsed() < SERVER_STOP_TIMEOUT {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_gates_bound_both_ends() {
        let gate = |toml: &str, name: &str| {
            let table: toml::Table = toml.parse().unwrap();
            in_range(&table, &ProtocolVersion::from_name(name).unwrap())
        };
        assert_eq!(gate("", "1.20.1"), Ok(true));
        assert_eq!(gate("min_version = \"1.20.5\"", "1.20.5"), Ok(true));
        assert_eq!(gate("min_version = \"1.20.5\"", "26.3"), Ok(true));
        assert_eq!(gate("min_version = \"1.20.5\"", "1.20.4"), Ok(false));
        assert_eq!(gate("max_version = \"1.21.1\"", "1.21.3"), Ok(false));
        assert_eq!(gate("max_version = \"1.21.1\"", "1.20.1"), Ok(true));
        assert!(gate("min_version = \"1.8.9\"", "26.2").is_err());
    }

    #[test]
    fn scripts_resolve_by_folder_and_name() {
        let root = std::env::temp_dir().join(format!("ingame-test-scripts-{}", std::process::id()));
        for file in [
            "a/one.toml",
            "a/two.toml",
            "b/one.toml",
            "top.toml",
            "a/notes.md",
        ] {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        let names = |paths: Vec<PathBuf>| -> Vec<String> {
            paths.iter().map(|p| script_name(&root, p)).collect()
        };
        assert_eq!(
            names(scripts(&root, &[]).unwrap()),
            ["a/one", "a/two", "b/one", "top"]
        );
        assert_eq!(
            names(scripts(&root, &["a".into()]).unwrap()),
            ["a/one", "a/two"]
        );
        assert_eq!(names(scripts(&root, &["b/one".into()]).unwrap()), ["b/one"]);
        assert!(scripts(&root, &["c".into()]).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
