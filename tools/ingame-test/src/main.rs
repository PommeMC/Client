//! Runs pomme's in-game test scripts (`tests/ingame/**/*.toml`) against a local
//! vanilla 26.2 server: downloads and verifies the server jar once, starts it
//! offline with the test player opped, then runs a harness client per script.
//!
//! `ingame-test [--jobs N] [scripts...]` runs the named scripts or folders
//! (all when none, e.g. `entities` or `entities/data`) on N workers, each
//! with its own server and world;
//! `ingame-test --server-only` starts the server for live socket sessions.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitCode, Stdio};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use md5::Digest as _;

/// The version the server runs and the client joins as.
const VERSION: &str = "26.2";
/// The 26.2 server's sha1 from its `version.json` (`downloads.server`),
/// which also names its download.
const SERVER_SHA1: &str = "823e2250d24b3ddac457a60c92a6a941943fcd6a";
/// 26.2's `javaVersion.majorVersion`.
const JAVA_MAJOR: u32 = 25;
const PLAYER: &str = "Tester";
const HOST: &str = "127.0.0.1";
const SERVER_START_TIMEOUT: Duration = Duration::from_secs(180);
const SERVER_STOP_TIMEOUT: Duration = Duration::from_secs(30);
const PLAYER_LEAVE_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_JOBS: usize = 6;

const OUT_DIR: &str = "target/ingame";
const SCRIPT_DIR: &str = "tests/ingame";

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // Just the verified server jar, for tools that run vanilla's code.
    if take_flag(&mut args, "--fetch-server") {
        return match server_jar(&Path::new(OUT_DIR).join("server")) {
            Ok(jar) => {
                println!("{}", jar.display());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("ingame-test: {e}");
                ExitCode::from(2)
            }
        };
    }
    let server_only = take_flag(&mut args, "--server-only");
    let jobs = match take_value(&mut args, "--jobs").map(|n| n.parse::<usize>()) {
        None => DEFAULT_JOBS,
        Some(Ok(n)) if n > 0 => n,
        Some(_) => {
            eprintln!("ingame-test: --jobs takes a positive number");
            return ExitCode::from(2);
        }
    };
    match run(server_only, jobs, &args) {
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

/// Whether every script passed.
fn run(server_only: bool, jobs: usize, names: &[String]) -> Result<bool, String> {
    let scripts = scripts(Path::new(SCRIPT_DIR), names)?;
    let server_dir = Path::new(OUT_DIR).join("server");
    let jar = server_jar(&server_dir)?;
    let java = java()?;

    if server_only {
        let mut server = Server::start(&java, &jar, &server_dir)?;
        println!("Vanilla server ready on {HOST}:{}", server.port);
        println!(
            "Join with: just client-dev --features test-harness -- {} --harness-port <port>",
            join_args(server.port).join(" ")
        );
        server.wait();
        return Ok(true);
    }

    build_client()?;
    let queue = Mutex::new(scripts.iter());
    let failed = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..jobs.min(scripts.len()))
            .map(|n| {
                let (queue, failed, java, jar) = (&queue, &failed, &java, &jar);
                scope.spawn(move || -> Result<(), String> {
                    let dir = Path::new(OUT_DIR).join("workers").join(n.to_string());
                    let mut server = Server::start(java, jar, &dir)?;
                    let game_dir = dir.join("client");
                    loop {
                        let next = queue.lock().unwrap().next();
                        let Some(script) = next else {
                            return Ok(());
                        };
                        if !run_script(script, server.port, &game_dir)? {
                            failed.lock().unwrap().push(script.display().to_string());
                        }
                        server.reset_player()?;
                    }
                })
            })
            .collect();
        workers
            .into_iter()
            .try_for_each(|worker| worker.join().expect("worker panicked"))
    })?;
    let mut failed = failed.into_inner().unwrap();
    failed.sort();
    println!(
        "\n{} of {} scripts passed",
        scripts.len() - failed.len(),
        scripts.len()
    );
    for script in &failed {
        println!("  FAILED {script}");
    }
    Ok(failed.is_empty())
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

/// The verified server jar, downloaded on first use.
fn server_jar(dir: &Path) -> Result<PathBuf, String> {
    let jar = dir.join("server.jar");
    if std::fs::read(&jar).is_ok_and(|bytes| sha1(&bytes) == SERVER_SHA1) {
        return Ok(jar);
    }
    println!("Downloading the vanilla {VERSION} server...");
    let url = format!("https://piston-data.mojang.com/v1/objects/{SERVER_SHA1}/server.jar");
    let bytes = reqwest::blocking::get(url)
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.bytes())
        .map_err(|e| format!("downloading the server: {e}"))?;
    if sha1(&bytes) != SERVER_SHA1 {
        return Err("the downloaded server jar doesn't match its sha1".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    std::fs::write(&jar, &bytes).map_err(|e| e.to_string())?;
    Ok(jar)
}

fn sha1(bytes: &[u8]) -> String {
    sha1_smol::Sha1::from(bytes).digest().to_string()
}

/// `$JAVA_HOME/bin/java`, else `java` on PATH, checked to be Java 25+.
fn java() -> Result<PathBuf, String> {
    let java = std::env::var_os("JAVA_HOME")
        .map(|home| Path::new(&home).join("bin").join("java"))
        .unwrap_or_else(|| "java".into());
    let output = Command::new(&java).arg("-version").output().map_err(|e| {
        format!(
            "running {}: {e} (install Java {JAVA_MAJOR})",
            java.display()
        )
    })?;
    // `openjdk version "25.0.2" ...` on stderr.
    let banner = String::from_utf8_lossy(&output.stderr);
    let major = banner
        .split('"')
        .nth(1)
        .and_then(|v| v.split('.').next()?.parse::<u32>().ok());
    match major {
        Some(major) if major >= JAVA_MAJOR => Ok(java),
        _ => Err(format!(
            "{VERSION}'s server needs Java {JAVA_MAJOR}+, found: {}",
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

/// Runs one script in a harness client, logging to `client.log` in its
/// output dir; whether it passed.
fn run_script(script: &Path, port: u16, game_dir: &Path) -> Result<bool, String> {
    let name = script_name(Path::new(SCRIPT_DIR), script);
    let out = Path::new(OUT_DIR).join(&name);
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let log_path = out.join("client.log");
    let log = std::fs::File::create(&log_path).map_err(|e| e.to_string())?;
    let client =
        Path::new("target/debug").join(format!("pomme-client{}", std::env::consts::EXE_SUFFIX));
    let started = Instant::now();
    let status = Command::new(&client)
        .args(join_args(port))
        .arg("--game-dir")
        .arg(game_dir)
        .arg("--harness-script")
        .arg(script)
        .arg("--harness-out")
        .arg(&out)
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log)
        .status()
        .map_err(|e| format!("starting {}: {e}", client.display()))?;
    let secs = started.elapsed().as_secs_f32();
    let passed = status.success();
    if passed {
        println!("PASS {name} ({secs:.1}s)");
    } else {
        println!("FAIL {name} ({secs:.1}s), see {}", log_path.display());
    }
    Ok(passed)
}

/// The client flags that join the test server as the test player.
fn join_args(port: u16) -> Vec<String> {
    [
        "--version",
        VERSION,
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
        let jar = jar.canonicalize().map_err(|e| e.to_string())?;
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
        Ok(())
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
