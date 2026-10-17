//! Which project a runtime process belongs to.
//!
//! Every Node.js process on a machine is the same `node.exe`, and so was one row: three worktrees
//! each running `pnpm dev` showed up as a single "Node.js JavaScript Runtime" holding three ports,
//! with no way to tell which was which. A runtime's script path, or failing that its working
//! directory, says which project it is serving; the nearest folder above it with a project file
//! in it names the row.

use std::path::{Path, PathBuf};

/// Interpreters and runtimes whose processes are only meaningful per project.
const RUNTIMES: &[&str] = &[
    "node.exe", "deno.exe", "bun.exe", "python.exe", "pythonw.exe", "python3.exe", "java.exe", "javaw.exe",
    "dotnet.exe", "ruby.exe", "php.exe", "perl.exe",
];

/// What makes a folder a project.
const MARKERS: &[&str] = &[
    "package.json", "deno.json", "pyproject.toml", "requirements.txt", "setup.py", "Pipfile", "Cargo.toml", "go.mod",
    "pom.xml", "build.gradle", "build.gradle.kts", "Gemfile", "composer.json", ".git",
];

/// How far up from a script or working directory to look for a project file.
const MAX_LEVELS: usize = 8;

pub fn is_runtime(image: &str) -> bool {
    RUNTIMES.iter().any(|name| image.eq_ignore_ascii_case(name))
}

/// The project folder a runtime process is working for, if one can be told.
///
/// The script comes first: `node E:\work\site\node_modules\vite\bin\vite.js` belongs to
/// `E:\work\site` however it was started. A script under a global tool folder (`npm`'s own
/// `node_modules`, a Python installation) says nothing about the project, and then the working
/// directory decides, which is where `pnpm dev` and `python -m` run from.
pub fn project_root(args: &[String], cwd: Option<&str>, has_marker: &dyn Fn(&Path) -> bool, home: Option<&Path>) -> Option<PathBuf> {
    let cwd = cwd.map(|c| PathBuf::from(c.trim_end_matches(['\\', '/'])));
    let script = args.iter().skip(1).find(|arg| looks_like_script(arg)).map(|arg| {
        let path = PathBuf::from(arg);
        let full = match (&cwd, path.is_relative()) {
            (Some(dir), true) => dir.join(path),
            _ => path,
        };
        normalize(&full)
    });
    let candidates = script.iter().filter_map(|s| from_script(s, has_marker, home)).chain(
        cwd.iter().filter_map(|dir| nearest_marked(dir, has_marker, home)),
    );
    candidates.into_iter().next()
}

fn looks_like_script(arg: &str) -> bool {
    if arg.starts_with('-') {
        return false;
    }
    let lower = arg.to_ascii_lowercase();
    [".js", ".mjs", ".cjs", ".ts", ".mts", ".py", ".pyw", ".rb", ".php", ".pl", ".jar", ".dll"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

fn from_script(script: &Path, has_marker: &dyn Fn(&Path) -> bool, home: Option<&Path>) -> Option<PathBuf> {
    let text = script.to_string_lossy();
    let lower = text.to_ascii_lowercase();
    // The project is the folder that installed the package, not the package itself.
    if let Some(at) = lower.find("\\node_modules\\").or_else(|| lower.find("/node_modules/")) {
        let owner = PathBuf::from(&text[..at]);
        // `npx` and `pnpm dlx` install into a cache folder named by a hash, which says nothing;
        // the package they ran (an MCP server, a CLI) is the thing to name the row after.
        if lower[..at].contains("\\_npx\\") || lower[..at].contains("\\dlx\\") {
            return package_dir(&text[at + "\\node_modules\\".len()..]).map(|package| owner.join("node_modules").join(package));
        }
        return (has_marker(&owner) && Some(owner.as_path()) != home).then_some(owner);
    }
    nearest_marked(script.parent()?, has_marker, home)
}

/// Resolves `.` and `..` and doubled separators without touching the disk. npm's shims start
/// packages as `…\node_modules\.bin\\..\pkg\cli.js`, which has to read as `…\node_modules\pkg`.
fn normalize(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let mut parts: Vec<&str> = Vec::new();
    for part in text.split(['\\', '/']) {
        match part {
            "" | "." => {}
            ".." => {
                // Never climb above the drive.
                if parts.len() > 1 {
                    parts.pop();
                }
            }
            other => parts.push(other),
        }
    }
    let unc = text.starts_with(r"\\");
    let joined = parts.join("\\");
    PathBuf::from(if unc { format!(r"\\{joined}") } else if parts.len() == 1 { format!("{joined}\\") } else { joined })
}

/// `@scope\name\bin\cli.js` → `@scope\name`; `name\dist\index.js` → `name`.
fn package_dir(inside: &str) -> Option<PathBuf> {
    let mut parts = inside.split(['\\', '/']).filter(|p| !p.is_empty());
    let first = parts.next()?;
    if first.starts_with('@') {
        Some(PathBuf::from(first).join(parts.next()?))
    } else {
        Some(PathBuf::from(first))
    }
}

fn nearest_marked(start: &Path, has_marker: &dyn Fn(&Path) -> bool, home: Option<&Path>) -> Option<PathBuf> {
    let mut dir = Some(start);
    for _ in 0..MAX_LEVELS {
        let here = dir?;
        // The home folder often holds a stray package.json or .git; it is not a project.
        if Some(here) == home || here.parent().is_none() {
            return None;
        }
        if has_marker(here) {
            return Some(here.to_path_buf());
        }
        dir = here.parent();
    }
    None
}

/// The real file system check.
pub fn has_project_file(dir: &Path) -> bool {
    MARKERS.iter().any(|marker| dir.join(marker).exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// A fake disk: these folders are projects.
    fn projects(list: &'static [&'static str]) -> impl Fn(&Path) -> bool {
        move |dir: &Path| list.iter().any(|p| Path::new(p) == dir)
    }

    const HOME: &str = r"C:\Users\me";

    #[test]
    fn a_package_script_belongs_to_the_project_that_installed_it() {
        let marked = projects(&[r"E:\work\site"]);
        let root = project_root(
            &args(&[r"C:\nodejs\node.exe", r"E:\work\site\node_modules\vite\bin\vite.js", "--port", "1420"]),
            Some(r"E:\work\site"),
            &marked,
            Some(Path::new(HOME)),
        );
        assert_eq!(root.as_deref(), Some(Path::new(r"E:\work\site")));
    }

    #[test]
    fn a_global_tool_falls_back_to_the_working_directory() {
        // pnpm's own script lives in the user's npm folder; the project is where it was run.
        let marked = projects(&[r"E:\work\api"]);
        let root = project_root(
            &args(&[r"C:\nodejs\node.exe", r"C:\Users\me\AppData\Roaming\npm\node_modules\pnpm\bin\pnpm.cjs", "dev"]),
            Some(r"E:\work\api\"),
            &marked,
            Some(Path::new(HOME)),
        );
        assert_eq!(root.as_deref(), Some(Path::new(r"E:\work\api")));
    }

    /// Measured on this machine: four MCP servers started with `npx -y` showed up named after
    /// the cache folder's hash.
    #[test]
    fn a_package_run_with_npx_is_named_after_the_package() {
        let cache = r"C:\Users\me\AppData\Local\npm-cache\_npx\99336612077b7094";
        let scoped = format!(r"{cache}\node_modules\@modelcontextprotocol\server-filesystem\dist\index.js");
        let root = project_root(&args(&["node.exe", &scoped]), Some(HOME), &projects(&[]), Some(Path::new(HOME)));
        assert_eq!(root, Some(PathBuf::from(format!(r"{cache}\node_modules\@modelcontextprotocol\server-filesystem"))));
        let plain = format!(r"{cache}\node_modules\some-cli\bin\cli.js");
        let root = project_root(&args(&["node.exe", &plain]), None, &projects(&[]), None);
        assert_eq!(root.as_deref().and_then(Path::file_name).and_then(|n| n.to_str()), Some("some-cli"));
        // The exact shape npm's shim produced on this machine.
        let shim = format!(r"{cache}\node_modules\.bin\\..\xcodebuildmcp\build\cli.js");
        let root = project_root(&args(&["node", &shim, "mcp"]), None, &projects(&[]), None);
        assert_eq!(root.as_deref().and_then(Path::file_name).and_then(|n| n.to_str()), Some("xcodebuildmcp"));
    }

    #[test]
    fn paths_are_normalised_without_climbing_above_the_drive() {
        assert_eq!(normalize(Path::new(r"C:\a\.bin\\..\b\.\c.js")), PathBuf::from(r"C:\a\b\c.js"));
        assert_eq!(normalize(Path::new(r"C:\..\..\x")), PathBuf::from(r"C:\x"));
        assert_eq!(normalize(Path::new(r"\\server\share\..\other\f.js")), PathBuf::from(r"\\server\other\f.js"));
    }

    #[test]
    fn a_relative_script_is_resolved_against_the_working_directory() {
        let marked = projects(&[r"D:\py\bot"]);
        let root = project_root(&args(&["python.exe", r"src\main.py"]), Some(r"D:\py\bot"), &marked, None);
        assert_eq!(root.as_deref(), Some(Path::new(r"D:\py\bot")));
    }

    #[test]
    fn the_home_folder_and_the_drive_root_are_not_projects() {
        let marked = projects(&[HOME, r"C:\"]);
        assert_eq!(project_root(&args(&["node.exe"]), Some(HOME), &marked, Some(Path::new(HOME))), None);
        assert_eq!(project_root(&args(&["node.exe", r"C:\x.js"]), Some(r"C:\"), &marked, None), None);
    }

    #[test]
    fn flags_are_not_scripts() {
        let marked = projects(&[r"E:\w"]);
        let root = project_root(&args(&["node.exe", "--max-old-space-size=4096.js"]), Some(r"E:\w"), &marked, None);
        assert_eq!(root.as_deref(), Some(Path::new(r"E:\w")), "the working directory decides");
    }

    #[test]
    fn runtimes_are_recognised_by_image_name() {
        assert!(is_runtime("node.exe"));
        assert!(is_runtime("Python.EXE"));
        assert!(!is_runtime("chrome.exe"));
    }
}
