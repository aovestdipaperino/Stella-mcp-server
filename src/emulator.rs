//! Locating the toolchain, the headless helper, and building it on demand.
//!
//! The MCP server drives a headless Gopher2600 helper (`gopher2600-mcp`). This
//! module resolves where that binary, the Go toolchain, and an optional full
//! Gopher2600 (for visual play) live, and can build the helper via
//! `scripts/build-emu.sh`.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result, anyhow, bail};
use tokio::process::Command;

/// Resolved locations and versions used by the server.
#[derive(Debug, Clone)]
pub struct Toolchain {
    /// Project root (where `emu/`, `scripts/`, and `bin/` live).
    pub root: PathBuf,
    /// The Go binary, if found on `PATH`.
    pub go: Option<PathBuf>,
    /// The built headless helper, if present.
    pub helper: Option<PathBuf>,
    /// A full (windowed) Gopher2600 build, if found.
    pub stock: Option<PathBuf>,
    /// The Stella emulator, if found.
    pub stella: Option<PathBuf>,
    /// The DASM assembler, used to build ROMs from source, if found.
    pub dasm: Option<PathBuf>,
    /// The GitHub CLI, used to fetch gists, if found.
    pub gh: Option<PathBuf>,
}

impl Toolchain {
    /// Resolves the toolchain from the environment and project layout.
    #[must_use]
    pub fn discover() -> Self {
        let root = project_root();
        Self {
            go: which("go"),
            helper: helper_path(&root),
            stock: stock_path(),
            stella: stella_path(),
            dasm: env_or_which("DASM", "dasm"),
            gh: env_or_which("GH", "gh"),
            root,
        }
    }

    /// A windowed emulator for visual play, preferring Stella over Gopher2600.
    #[must_use]
    pub fn windowed_player(&self) -> Option<(PathBuf, &'static str)> {
        if let Some(p) = &self.stella {
            return Some((p.clone(), "Stella"));
        }
        self.stock.as_ref().map(|p| (p.clone(), "Gopher2600"))
    }

    /// Directory of bundled DASM headers (`vcs.h`, `macro.h`) for 8bitworkshop sources.
    #[must_use]
    pub fn dasm_include_dir(&self) -> PathBuf {
        self.root.join("assets/dasm")
    }

    /// The pinned Gopher2600 version recorded in `emu/go.mod`, if readable.
    #[must_use]
    pub fn gopher2600_version(&self) -> Option<String> {
        let go_mod = std::fs::read_to_string(self.root.join("emu/go.mod")).ok()?;
        go_mod.lines().find_map(|l| {
            let l = l.trim();
            l.strip_prefix("require github.com/jetsetilly/gopher2600 ")
                .map(|v| v.trim().to_string())
        })
    }

    /// Builds the helper by running `scripts/build-emu.sh`.
    ///
    /// # Errors
    ///
    /// Returns an error if Go is missing or the build script exits non-zero;
    /// the script's combined output is included in the error.
    pub async fn build_helper(&self) -> Result<String> {
        if self.go.is_none() {
            bail!("Go toolchain not found on PATH; install it (e.g. `brew install go`)");
        }
        let script = self.root.join("scripts/build-emu.sh");
        let output = Command::new("bash")
            .arg(&script)
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .output()
            .await
            .with_context(|| format!("running {}", script.display()))?;

        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if !output.status.success() {
            bail!("build-emu.sh failed:\n{combined}");
        }
        Ok(combined)
    }

    /// Result of assembling a ROM from source.
    ///
    /// Holds the compiled ROM path, a sibling DASM symbol file (auto-loaded by
    /// the debugger), and the assembler's combined log output.
    pub async fn build_rom(
        &self,
        source: &Path,
        output: Option<&Path>,
        format: u8,
    ) -> Result<BuildOutput> {
        let dasm = self.dasm.as_ref().ok_or_else(|| {
            anyhow!("DASM assembler not found on PATH; install it (e.g. `brew install dasm`)")
        })?;
        if !source.is_file() {
            bail!("source not found: {}", source.display());
        }

        let rom = match output {
            Some(p) => p.to_path_buf(),
            None => source.with_extension("bin"),
        };
        let symbols = rom.with_extension("sym");

        // -f<format> (3 = raw), -o output, -s DASM symbol file (the debugger
        // auto-loads <rom>.sym), and include dirs for the bundled 8bitworkshop
        // headers plus the source's own directory.
        let mut cmd = Command::new(dasm);
        cmd.arg(source)
            .arg(format!("-f{format}"))
            .arg(format!("-o{}", rom.display()))
            .arg(format!("-s{}", symbols.display()))
            .arg(format!("-I{}", self.dasm_include_dir().display()));
        if let Some(dir) = source.parent().filter(|d| !d.as_os_str().is_empty()) {
            cmd.arg(format!("-I{}", dir.display()));
        }

        let out = cmd
            .stdin(Stdio::null())
            .output()
            .await
            .context("running dasm")?;
        let log = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        // DASM exits 0 on success; it leaves no output file on failure.
        if !out.status.success() || !rom.is_file() {
            bail!("dasm failed to assemble {}:\n{log}", source.display());
        }
        let size = std::fs::metadata(&rom).map_or(0, |m| m.len());
        Ok(BuildOutput {
            rom,
            symbols,
            size,
            log,
        })
    }

    /// Fetches a gist file to a local directory using the GitHub CLI.
    ///
    /// `gist` is a gist id or a substring matched against the caller's gist
    /// descriptions and file names. `file` selects one file when the gist has
    /// several. Returns the local path of the written file.
    pub async fn fetch_gist(
        &self,
        gist: &str,
        file: Option<&str>,
        dest_dir: &Path,
    ) -> Result<PathBuf> {
        let gh = self
            .gh
            .as_ref()
            .ok_or_else(|| anyhow!("GitHub CLI not found on PATH; install it (e.g. `brew install gh`) and run `gh auth login`"))?;

        let id = self.resolve_gist_id(gh, gist).await?;

        // ask the API for the file list so we can pick the right one.
        let names = self.gist_files(gh, &id).await?;
        if names.is_empty() {
            bail!("gist {id} has no files");
        }
        let chosen = match file {
            Some(f) => names
                .iter()
                .find(|n| n.eq_ignore_ascii_case(f))
                .cloned()
                .ok_or_else(|| {
                    anyhow!("file '{f}' not in gist {id}; files: {}", names.join(", "))
                })?,
            None if names.len() == 1 => names[0].clone(),
            None => bail!(
                "gist {id} has multiple files; pass `file`: {}",
                names.join(", ")
            ),
        };

        let raw = run_capture(
            gh,
            &["gist", "view", &id, "-f", &chosen, "--raw"],
            "gh gist view",
        )
        .await?;
        std::fs::create_dir_all(dest_dir).context("creating gist destination")?;
        let path = dest_dir.join(&chosen);
        std::fs::write(&path, raw).context("writing gist file")?;
        Ok(path)
    }

    /// Resolves a gist reference (id or search string) to a concrete gist id.
    async fn resolve_gist_id(&self, gh: &Path, gist: &str) -> Result<String> {
        // a bare hex token of gist-id length is taken as an id.
        if gist.len() >= 20 && gist.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(gist.to_string());
        }
        let json = run_capture(
            gh,
            &[
                "api",
                "--paginate",
                "/gists?per_page=100",
                "--jq",
                ".[] | {id, description, files: (.files | keys)}",
            ],
            "gh api gists",
        )
        .await?;
        let needle = gist.to_lowercase();
        let mut matches: Vec<String> = Vec::new();
        for line in json.lines().filter(|l| !l.trim().is_empty()) {
            let v: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let desc = v.get("description").and_then(|d| d.as_str()).unwrap_or("");
            let files = v
                .get("files")
                .and_then(|f| f.as_array())
                .map_or_else(String::new, |a| {
                    a.iter()
                        .filter_map(|x| x.as_str())
                        .collect::<Vec<_>>()
                        .join(" ")
                });
            if format!("{desc} {files}").to_lowercase().contains(&needle)
                && let Some(id) = v.get("id").and_then(|i| i.as_str())
            {
                matches.push(id.to_string());
            }
        }
        match matches.len() {
            1 => Ok(matches.remove(0)),
            0 => bail!("no gist matched '{gist}'"),
            n => bail!("'{gist}' matched {n} gists; pass an exact id"),
        }
    }

    /// Returns the file names contained in a gist.
    async fn gist_files(&self, gh: &Path, id: &str) -> Result<Vec<String>> {
        let json = run_capture(
            gh,
            &["api", &format!("/gists/{id}"), "--jq", ".files | keys[]"],
            "gh api gist",
        )
        .await?;
        Ok(json
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect())
    }
}

/// A successful ROM build.
#[derive(Debug, Clone)]
pub struct BuildOutput {
    /// The compiled ROM image.
    pub rom: PathBuf,
    /// The DASM symbol file written alongside the ROM.
    pub symbols: PathBuf,
    /// Size of the ROM in bytes.
    pub size: u64,
    /// The assembler's combined stdout/stderr.
    pub log: String,
}

/// Runs a command and returns its stdout, erroring (with stderr) on failure.
async fn run_capture(bin: &Path, args: &[&str], what: &str) -> Result<String> {
    let out = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .await
        .with_context(|| format!("running {what}"))?;
    if !out.status.success() {
        bail!(
            "{what} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Returns the project root, preferring `STELLA_MCP_ROOT`, else the compile-time
/// manifest directory.
fn project_root() -> PathBuf {
    std::env::var_os("STELLA_MCP_ROOT")
        .map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")), PathBuf::from)
}

/// Resolves the headless helper: `STELLA_MCP_EMU`, then `bin/`, then `PATH`.
fn helper_path(root: &Path) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("STELLA_MCP_EMU").map(PathBuf::from)
        && p.is_file()
    {
        return Some(p);
    }
    let local = root.join("bin/gopher2600-mcp");
    if local.is_file() {
        return Some(local);
    }
    which("gopher2600-mcp")
}

/// Resolves a full Gopher2600 build for visual play: `GOPHER2600`, then `PATH`.
fn stock_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("GOPHER2600").map(PathBuf::from)
        && p.is_file()
    {
        return Some(p);
    }
    which("gopher2600")
}

/// Resolves the Stella emulator: `STELLA`, then `PATH`, then the macOS app bundle.
fn stella_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("STELLA").map(PathBuf::from)
        && p.is_file()
    {
        return Some(p);
    }
    if let Some(p) = which("stella") {
        return Some(p);
    }
    let app = PathBuf::from("/Applications/Stella.app/Contents/MacOS/Stella");
    app.is_file().then_some(app)
}

/// Resolves a tool by an explicit env-var path, falling back to `PATH` lookup.
fn env_or_which(env: &str, name: &str) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(env).map(PathBuf::from)
        && p.is_file()
    {
        return Some(p);
    }
    which(name)
}

/// Finds an executable by name on `PATH`.
fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(name);
        candidate.is_file().then_some(candidate)
    })
}
