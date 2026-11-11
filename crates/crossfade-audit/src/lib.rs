//! Workspace policy checks run as tests. Not part of the engine and never a
//! dependency of it.
#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::process::Command;

use serde::Deserialize;

/// The subset of `cargo metadata --format-version 1` the audit reads.
#[derive(Debug, Clone, Deserialize)]
pub struct Metadata {
    pub packages: Vec<Package>,
    pub workspace_members: Vec<String>,
    pub resolve: Option<Resolve>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Package {
    pub name: String,
    pub id: String,
    pub links: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Resolve {
    pub nodes: Vec<Node>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Node {
    pub id: String,
    pub deps: Vec<NodeDep>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NodeDep {
    pub pkg: String,
    pub dep_kinds: Vec<DepKind>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DepKind {
    /// `None` for normal dependencies, else `"dev"` or `"build"`.
    pub kind: Option<String>,
}

impl Metadata {
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

/// The crates that must stay pure Rust under every feature.
pub const ENGINE_CRATES: &[&str] = &[
    "crossfade-core",
    "crossfade-format",
    "crossfade-codec",
    "crossfade-flac",
    "crossfade-loudness",
    "crossfade-resample",
    "crossfade-dsp",
    "crossfade-graph",
];

/// Exemptions from the purity gate. Every entry needs a reason a reviewer can
/// check; keep it empty unless something genuinely cannot be avoided.
pub const ALLOWLIST: &[Allowed] = &[];

/// Crates whose only purpose is building or finding native code.
pub const NATIVE_BUILD_HELPERS: &[&str] = &[
    "cc",
    "cmake",
    "bindgen",
    "pkg-config",
    "nasm-rs",
    "autotools",
    "vcpkg",
];

fn classify(package: &Package) -> Option<Reason> {
    if let Some(links) = &package.links {
        Some(Reason::Links(links.clone()))
    } else if NATIVE_BUILD_HELPERS.contains(&package.name.as_str()) {
        Some(Reason::NativeBuildHelper)
    } else if package.name.ends_with("-sys") {
        Some(Reason::SysCrate)
    } else {
        None
    }
}

/// Runs `cargo metadata` with all features on this workspace. Without
/// `--filter-platform`, every target's dependencies are included.
pub fn workspace_metadata() -> Result<Metadata, String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml");
    let output = Command::new(&cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--all-features",
            "--locked",
        ])
        .args(["--manifest-path", manifest])
        .output()
        .map_err(|e| format!("could not run {cargo}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let json = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;
    Metadata::from_json(&json).map_err(|e| format!("unexpected cargo metadata: {e}"))
}

/// A package exempted from the audit, with the reason it is acceptable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Allowed {
    pub package: &'static str,
    pub reason: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// Declares `links`, so it links a native library.
    Links(String),
    /// A crate whose job is compiling or locating native code.
    NativeBuildHelper,
    /// Named `*-sys`, the convention for native-library bindings.
    SysCrate,
    /// A requested root is not a workspace member, so nothing was checked.
    NotInWorkspace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub package: String,
    pub reason: Reason,
    /// Package names from the root that first reaches the offender to it.
    pub path: Vec<String>,
}

/// Walks the dependency closure of each root and reports every package that
/// breaks the pure-Rust rule, once, with the first path that reaches it.
pub fn audit(metadata: &Metadata, roots: &[&str], allowlist: &[Allowed]) -> Vec<Violation> {
    let packages: HashMap<&str, &Package> = metadata
        .packages
        .iter()
        .map(|p| (p.id.as_str(), p))
        .collect();
    let edges: HashMap<&str, Vec<&str>> = metadata
        .resolve
        .iter()
        .flat_map(|r| &r.nodes)
        .map(|n| {
            let deps = n
                .deps
                .iter()
                .filter(|d| d.dep_kinds.iter().any(|k| k.kind.as_deref() != Some("dev")))
                .map(|d| d.pkg.as_str())
                .collect();
            (n.id.as_str(), deps)
        })
        .collect();

    let mut seen = HashSet::new();
    let mut violations = Vec::new();
    for root in roots {
        let Some(root_id) = metadata
            .workspace_members
            .iter()
            .map(String::as_str)
            .find(|id| packages.get(id).is_some_and(|p| p.name == *root))
        else {
            violations.push(Violation {
                package: (*root).to_owned(),
                reason: Reason::NotInWorkspace,
                path: Vec::new(),
            });
            continue;
        };
        let mut parent: HashMap<&str, &str> = HashMap::new();
        let mut queue = VecDeque::from([root_id]);
        seen.insert(root_id);
        while let Some(id) = queue.pop_front() {
            let package = packages[id];
            let allowed = allowlist.iter().any(|a| a.package == package.name);
            // An allowed package's own dependencies are still audited.
            if let Some(reason) = classify(package).filter(|_| !allowed) {
                let mut path = vec![package.name.clone()];
                let mut cursor = id;
                while let Some(&up) = parent.get(cursor) {
                    path.push(packages[up].name.clone());
                    cursor = up;
                }
                path.reverse();
                violations.push(Violation {
                    package: package.name.clone(),
                    reason,
                    path,
                });
            }
            for &dep in edges.get(id).into_iter().flatten() {
                if seen.insert(dep) {
                    parent.insert(dep, id);
                    queue.push_back(dep);
                }
            }
        }
    }
    violations
}
