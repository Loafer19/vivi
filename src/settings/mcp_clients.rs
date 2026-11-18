//! Detect Claude Desktop, Cursor and VS Code and merge a `vivi-mcp` entry into
//! their MCP JSON config (Settings → Controls → Remotes → Connect to…).

use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

/// MCP clients Vivi can write a `vivi-mcp` entry into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Client {
    ClaudeDesktop,
    Cursor,
    VsCode,
}

/// Operating system whose well-known config paths should be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Os {
    Linux,
    Mac,
    Windows,
}

/// Directories used to resolve client config paths (overridable in tests).
#[derive(Debug, Clone)]
pub(crate) struct FsRoots {
    pub home: PathBuf,
    /// `XDG_CONFIG_HOME`, or `~/.config`.
    pub config: PathBuf,
    /// Windows `%APPDATA%` or macOS `~/Library/Application Support`.
    pub app_data: PathBuf,
    /// Windows `%LOCALAPPDATA%`.
    pub local_app_data: PathBuf,
    /// macOS `/Applications` or Windows `%LOCALAPPDATA%\Programs`.
    pub applications: PathBuf,
}

/// How to launch `vivi-mcp` from the client's config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandSpec {
    pub command: String,
    pub args: Vec<String>,
}

/// A client whose config (or install) was found on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Detected {
    pub client: Client,
    pub config: PathBuf,
    pub connected: bool,
}

/// Result of merging a `vivi` entry into a config document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Merge {
    Added,
    AlreadyPresent,
}

#[derive(Debug)]
pub(crate) enum Error {
    Io(std::io::Error),
    Json(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Json(e) => write!(f, "{e}"),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl Client {
    pub(crate) fn all() -> [Self; 3] {
        [Self::ClaudeDesktop, Self::Cursor, Self::VsCode]
    }

    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::ClaudeDesktop => "claude",
            Self::Cursor => "cursor",
            Self::VsCode => "vscode",
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::ClaudeDesktop => "Claude Desktop",
            Self::Cursor => "Cursor",
            Self::VsCode => "VS Code",
        }
    }

    pub(crate) fn from_id(id: &str) -> Option<Self> {
        Self::all().into_iter().find(|c| c.id() == id)
    }

    /// Top-level JSON object that holds the server map.
    fn servers_key(self) -> &'static str {
        match self {
            Self::VsCode => "servers",
            Self::ClaudeDesktop | Self::Cursor => "mcpServers",
        }
    }
}

impl Os {
    pub(crate) fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

impl FsRoots {
    pub(crate) fn from_env() -> Self {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_default();
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let app_data = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| match Os::current() {
                Os::Windows => home.join("AppData").join("Roaming"),
                Os::Mac => home.join("Library").join("Application Support"),
                Os::Linux => config.clone(),
            });
        let local_app_data = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData").join("Local"));
        let applications = match Os::current() {
            Os::Mac => PathBuf::from("/Applications"),
            Os::Windows => local_app_data.join("Programs"),
            Os::Linux => PathBuf::from("/usr/share/applications"),
        };
        Self {
            home,
            config,
            app_data,
            local_app_data,
            applications,
        }
    }
}

impl CommandSpec {
    fn entry(&self, client: Client) -> Value {
        let mut m = Map::new();
        m.insert("command".into(), Value::String(self.command.clone()));
        if !self.args.is_empty() || client == Client::VsCode || client == Client::Cursor {
            m.insert(
                "args".into(),
                Value::Array(self.args.iter().cloned().map(Value::String).collect()),
            );
        }
        if client == Client::VsCode {
            m.insert("type".into(), Value::String("stdio".into()));
        }
        Value::Object(m)
    }
}

/// Path of the user's MCP config file for `client` on `os`.
pub(crate) fn config_path(client: Client, roots: &FsRoots, os: Os) -> PathBuf {
    match (client, os) {
        (Client::ClaudeDesktop, Os::Linux) => roots
            .config
            .join("Claude")
            .join("claude_desktop_config.json"),
        (Client::ClaudeDesktop, Os::Mac) => roots
            .app_data
            .join("Claude")
            .join("claude_desktop_config.json"),
        (Client::ClaudeDesktop, Os::Windows) => roots
            .app_data
            .join("Claude")
            .join("claude_desktop_config.json"),
        (Client::Cursor, _) => roots.home.join(".cursor").join("mcp.json"),
        (Client::VsCode, Os::Linux) => roots.config.join("Code").join("User").join("mcp.json"),
        (Client::VsCode, Os::Mac | Os::Windows) => {
            roots.app_data.join("Code").join("User").join("mcp.json")
        }
    }
}

fn installed(client: Client, roots: &FsRoots, os: Os) -> bool {
    let config = config_path(client, roots, os);
    if config.exists() || config.parent().is_some_and(|p| p.is_dir()) {
        return true;
    }
    match (client, os) {
        (Client::ClaudeDesktop, Os::Linux) => roots.config.join("Claude").is_dir(),
        (Client::ClaudeDesktop, Os::Mac) => roots.applications.join("Claude.app").is_dir(),
        (Client::ClaudeDesktop, Os::Windows) => {
            roots.applications.join("Claude").is_dir()
                || roots.local_app_data.join("Claude").is_dir()
        }
        (Client::Cursor, Os::Linux) => {
            roots.home.join(".cursor").is_dir()
                || roots.applications.join("cursor.desktop").is_file()
                || roots.home.join(".local/share/cursor").is_dir()
        }
        (Client::Cursor, Os::Mac) => {
            roots.home.join(".cursor").is_dir() || roots.applications.join("Cursor.app").is_dir()
        }
        (Client::Cursor, Os::Windows) => {
            roots.home.join(".cursor").is_dir()
                || roots.applications.join("cursor").is_dir()
                || roots.applications.join("Cursor").is_dir()
        }
        (Client::VsCode, Os::Linux) => {
            roots.config.join("Code").is_dir() || roots.applications.join("code.desktop").is_file()
        }
        (Client::VsCode, Os::Mac) => roots.applications.join("Visual Studio Code.app").is_dir(),
        (Client::VsCode, Os::Windows) => {
            roots.app_data.join("Code").is_dir()
                || roots.applications.join("Microsoft VS Code").is_dir()
        }
    }
}

/// Whether the document already has a `vivi` server under this client's key.
pub(crate) fn is_connected(text: &str, client: Client) -> bool {
    let Ok(Value::Object(root)) = serde_json::from_str::<Value>(text) else {
        return false;
    };
    root.get(client.servers_key())
        .and_then(Value::as_object)
        .is_some_and(|s| s.contains_key("vivi"))
}

/// Merge a `vivi` stdio server into `text` (empty / missing file → a new object).
/// Other keys and servers are kept. If `vivi` is already present it is left as-is.
pub(crate) fn merge_vivi_mcp(
    text: &str,
    client: Client,
    cmd: &CommandSpec,
) -> Result<(String, Merge), Error> {
    let trimmed = text.trim();
    let mut root = if trimmed.is_empty() {
        Value::Object(Map::new())
    } else {
        match serde_json::from_str::<Value>(trimmed) {
            Ok(Value::Object(m)) => Value::Object(m),
            Ok(_) => {
                return Err(Error::Json(
                    "the config file is JSON but not an object; left unchanged".into(),
                ));
            }
            Err(e) => {
                return Err(Error::Json(format!(
                    "the config file is not valid JSON: {e}"
                )));
            }
        }
    };
    let map = root.as_object_mut().expect("object");
    let key = client.servers_key();
    let servers = match map.get(key) {
        None => Map::new(),
        Some(Value::Object(m)) => m.clone(),
        Some(_) => {
            return Err(Error::Json(format!(
                "\"{key}\" is present but is not an object; left unchanged"
            )));
        }
    };
    if servers.contains_key("vivi") {
        return Ok((pretty(&root), Merge::AlreadyPresent));
    }
    let mut servers = servers;
    servers.insert("vivi".into(), cmd.entry(client));
    map.insert(key.into(), Value::Object(servers));
    Ok((pretty(&root), Merge::Added))
}

fn pretty(v: &Value) -> String {
    let mut s = serde_json::to_string_pretty(v).unwrap_or_else(|_| "{}".into());
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// Write a `vivi` entry into `path`, copying the existing file to `path.bak` first.
/// Idempotent: if `vivi` is already there, the file is not rewritten.
pub(crate) fn connect(path: &Path, client: Client, cmd: &CommandSpec) -> Result<Merge, Error> {
    let existing = if path.exists() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };
    let (out, merge) = merge_vivi_mcp(&existing, client, cmd)?;
    if merge == Merge::AlreadyPresent {
        return Ok(merge);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.exists() {
        let bak = bak_path(path);
        fs::copy(path, bak)?;
    }
    fs::write(path, out)?;
    Ok(Merge::Added)
}

fn bak_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".bak");
    PathBuf::from(s)
}

/// Clients whose config or install was found, with whether `vivi` is already in the file.
pub(crate) fn detect(roots: &FsRoots, os: Os) -> Vec<Detected> {
    Client::all()
        .into_iter()
        .filter(|c| installed(*c, roots, os))
        .map(|client| {
            let config = config_path(client, roots, os);
            let connected = config
                .exists()
                .then(|| fs::read_to_string(&config).ok())
                .flatten()
                .is_some_and(|t| is_connected(&t, client));
            Detected {
                client,
                config,
                connected,
            }
        })
        .collect()
}

pub(crate) fn detect_here() -> Vec<Detected> {
    detect(&FsRoots::from_env(), Os::current())
}

/// Locate `vivi-mcp` next to the running `vivi` binary, or fall back to `PATH`.
pub(crate) fn mcp_command() -> CommandSpec {
    if std::env::var("FLATPAK_ID").as_deref() == Ok("io.github.loafer19.vivi")
        || Path::new("/.flatpak-info").is_file()
            && std::env::var("FLATPAK_ID")
                .unwrap_or_default()
                .contains("vivi")
    {
        return CommandSpec {
            command: "flatpak".into(),
            args: vec![
                "run".into(),
                "--command=vivi-mcp".into(),
                "io.github.loafer19.vivi".into(),
            ],
        };
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(p) = sibling_mcp(&exe) {
            return CommandSpec {
                command: p.to_string_lossy().into_owned(),
                args: Vec::new(),
            };
        }
    }
    CommandSpec {
        command: "vivi-mcp".into(),
        args: Vec::new(),
    }
}

/// `vivi` / `vivi.exe` → `vivi-mcp` / `vivi-mcp.exe` in the same folder, if that file exists.
pub(crate) fn sibling_mcp(vivi: &Path) -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "vivi-mcp.exe"
    } else {
        "vivi-mcp"
    };
    let p = vivi.with_file_name(name);
    p.is_file().then_some(p)
}

/// Connect the named client (`claude`, `cursor`, `vscode`) on this machine.
pub(crate) fn connect_id(id: &str) -> Result<(Client, PathBuf, Merge), Error> {
    let client =
        Client::from_id(id).ok_or_else(|| Error::Json(format!("unknown MCP client {id}")))?;
    let roots = FsRoots::from_env();
    let path = config_path(client, &roots, Os::current());
    let merge = connect(&path, client, &mcp_command())?;
    Ok((client, path, merge))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn cmd() -> CommandSpec {
        CommandSpec {
            command: "/usr/bin/vivi-mcp".into(),
            args: Vec::new(),
        }
    }

    fn tmp() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "vivi-mcp-clients-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn roots(base: &Path) -> FsRoots {
        FsRoots {
            home: base.join("home"),
            config: base.join("config"),
            app_data: base.join("appdata"),
            local_app_data: base.join("local"),
            applications: base.join("apps"),
        }
    }

    #[test]
    fn paths_per_os() {
        let r = roots(Path::new("/r"));
        assert_eq!(
            config_path(Client::ClaudeDesktop, &r, Os::Linux),
            PathBuf::from("/r/config/Claude/claude_desktop_config.json")
        );
        assert_eq!(
            config_path(Client::ClaudeDesktop, &r, Os::Mac),
            PathBuf::from("/r/appdata/Claude/claude_desktop_config.json")
        );
        assert_eq!(
            config_path(Client::ClaudeDesktop, &r, Os::Windows),
            PathBuf::from("/r/appdata/Claude/claude_desktop_config.json")
        );
        assert_eq!(
            config_path(Client::Cursor, &r, Os::Linux),
            PathBuf::from("/r/home/.cursor/mcp.json")
        );
        assert_eq!(
            config_path(Client::Cursor, &r, Os::Windows),
            PathBuf::from("/r/home/.cursor/mcp.json")
        );
        assert_eq!(
            config_path(Client::VsCode, &r, Os::Linux),
            PathBuf::from("/r/config/Code/User/mcp.json")
        );
        assert_eq!(
            config_path(Client::VsCode, &r, Os::Mac),
            PathBuf::from("/r/appdata/Code/User/mcp.json")
        );
        assert_eq!(
            config_path(Client::VsCode, &r, Os::Windows),
            PathBuf::from("/r/appdata/Code/User/mcp.json")
        );
    }

    #[test]
    fn merge_into_empty_and_existing() {
        let (out, m) = merge_vivi_mcp("", Client::Cursor, &cmd()).unwrap();
        assert_eq!(m, Merge::Added);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["mcpServers"]["vivi"]["command"], "/usr/bin/vivi-mcp");
        assert_eq!(v["mcpServers"]["vivi"]["args"], json!([]));

        let existing = r#"{
  "mcpServers": {
    "other": { "command": "something" }
  },
  "keep": true
}"#;
        let (out, m) = merge_vivi_mcp(existing, Client::Cursor, &cmd()).unwrap();
        assert_eq!(m, Merge::Added);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["keep"], true);
        assert_eq!(v["mcpServers"]["other"]["command"], "something");
        assert_eq!(v["mcpServers"]["vivi"]["command"], "/usr/bin/vivi-mcp");
    }

    #[test]
    fn merge_is_idempotent() {
        let first = merge_vivi_mcp("", Client::ClaudeDesktop, &cmd()).unwrap().0;
        let (again, m) = merge_vivi_mcp(&first, Client::ClaudeDesktop, &cmd()).unwrap();
        assert_eq!(m, Merge::AlreadyPresent);
        assert_eq!(
            serde_json::from_str::<Value>(&again).unwrap()["mcpServers"]["vivi"]["command"],
            "/usr/bin/vivi-mcp"
        );
        // A different command does not clobber an existing vivi entry.
        let other = CommandSpec {
            command: "/opt/vivi-mcp".into(),
            args: Vec::new(),
        };
        let (third, m) = merge_vivi_mcp(&first, Client::ClaudeDesktop, &other).unwrap();
        assert_eq!(m, Merge::AlreadyPresent);
        assert_eq!(
            serde_json::from_str::<Value>(&third).unwrap()["mcpServers"]["vivi"]["command"],
            "/usr/bin/vivi-mcp"
        );
    }

    #[test]
    fn vscode_uses_servers_and_stdio_type() {
        let (out, _) = merge_vivi_mcp("", Client::VsCode, &cmd()).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v.get("mcpServers").is_none());
        assert_eq!(v["servers"]["vivi"]["type"], "stdio");
        assert_eq!(v["servers"]["vivi"]["command"], "/usr/bin/vivi-mcp");
    }

    #[test]
    fn merge_rejects_non_object() {
        assert!(merge_vivi_mcp("[]", Client::Cursor, &cmd()).is_err());
        assert!(merge_vivi_mcp("{\"mcpServers\": []}", Client::Cursor, &cmd()).is_err());
        assert!(merge_vivi_mcp("{", Client::Cursor, &cmd()).is_err());
    }

    #[test]
    fn connect_writes_backup_and_is_idempotent() {
        let base = tmp();
        let path = base.join("mcp.json");
        fs::write(
            &path,
            "{\n  \"mcpServers\": { \"other\": { \"command\": \"x\" } }\n}\n",
        )
        .unwrap();
        assert_eq!(
            connect(&path, Client::Cursor, &cmd()).unwrap(),
            Merge::Added
        );
        let bak = fs::read_to_string(base.join("mcp.json.bak")).unwrap();
        assert!(bak.contains("other"));
        assert!(!bak.contains("vivi"));
        let written = fs::read_to_string(&path).unwrap();
        assert!(is_connected(&written, Client::Cursor));
        assert!(written.contains("other"));

        let mtime = fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(
            connect(&path, Client::Cursor, &cmd()).unwrap(),
            Merge::AlreadyPresent
        );
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), mtime);

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn connect_creates_missing_file() {
        let base = tmp();
        let path = base.join("missing").join("mcp.json");
        assert_eq!(
            connect(&path, Client::Cursor, &cmd()).unwrap(),
            Merge::Added
        );
        assert!(path.is_file());
        assert!(!bak_path(&path).exists());
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn detect_finds_installed_and_connected() {
        let base = tmp();
        let r = roots(&base);
        fs::create_dir_all(r.home.join(".cursor")).unwrap();
        fs::create_dir_all(r.config.join("Code").join("User")).unwrap();
        fs::write(
            r.home.join(".cursor").join("mcp.json"),
            merge_vivi_mcp("", Client::Cursor, &cmd()).unwrap().0,
        )
        .unwrap();
        // VS Code is installed (User dir) but not yet connected.
        let found = detect(&r, Os::Linux);
        let names: Vec<_> = found.iter().map(|d| d.client).collect();
        assert_eq!(names, vec![Client::Cursor, Client::VsCode]);
        let cursor = found.iter().find(|d| d.client == Client::Cursor).unwrap();
        assert!(cursor.connected);
        let vs = found.iter().find(|d| d.client == Client::VsCode).unwrap();
        assert!(!vs.connected);
        // Empty tree: nothing found.
        let empty = tmp();
        assert!(detect(&roots(&empty), Os::Linux).is_empty());
        fs::remove_dir_all(&base).unwrap();
        fs::remove_dir_all(&empty).unwrap();
    }

    #[test]
    fn sibling_mcp_next_to_vivi() {
        let base = tmp();
        let vivi = base.join("vivi");
        let mcp = base.join("vivi-mcp");
        fs::write(&vivi, "").unwrap();
        assert!(sibling_mcp(&vivi).is_none());
        fs::write(&mcp, "").unwrap();
        assert_eq!(sibling_mcp(&vivi).as_deref(), Some(mcp.as_path()));
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn detect_macos_app_bundle() {
        let base = tmp();
        let mut r = roots(&base);
        r.applications = base.join("Applications");
        r.app_data = base.join("Library").join("Application Support");
        fs::create_dir_all(r.applications.join("Cursor.app")).unwrap();
        fs::create_dir_all(r.applications.join("Visual Studio Code.app")).unwrap();
        fs::create_dir_all(r.applications.join("Claude.app")).unwrap();
        let found = detect(&r, Os::Mac);
        assert_eq!(found.len(), 3);
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn flatpak_entry_keeps_args() {
        let cmd = CommandSpec {
            command: "flatpak".into(),
            args: vec![
                "run".into(),
                "--command=vivi-mcp".into(),
                "io.github.loafer19.vivi".into(),
            ],
        };
        let (out, _) = merge_vivi_mcp("", Client::ClaudeDesktop, &cmd).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["mcpServers"]["vivi"]["command"], "flatpak");
        assert_eq!(
            v["mcpServers"]["vivi"]["args"],
            json!(["run", "--command=vivi-mcp", "io.github.loafer19.vivi"])
        );
    }
}
