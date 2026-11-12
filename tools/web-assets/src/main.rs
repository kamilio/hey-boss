//! Rebuild the browser bundles, license notices and embedded diagram routes.
use serde_json::Value;
use std::{
    collections::BTreeSet,
    error::Error,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn append_licenses(root: &Path, metadata: &Path, bundle: &Path) -> Result<()> {
    let meta: Value = serde_json::from_slice(&fs::read(metadata)?)?;
    let modules = root.join("node_modules");
    let mut packages = BTreeSet::new();
    for input in meta["inputs"]
        .as_object()
        .ok_or("missing esbuild inputs")?
        .keys()
    {
        let input = root.join(input);
        if !input.starts_with(&modules) {
            continue;
        }
        let mut found = false;
        for dir in input
            .ancestors()
            .skip(1)
            .take_while(|p| p.starts_with(&modules))
        {
            if packages.contains(dir) {
                found = true;
                break;
            }
            let manifest = dir.join("package.json");
            if manifest.is_file() {
                let info: Value = serde_json::from_slice(&fs::read(manifest)?)?;
                if info["name"].is_string() {
                    packages.insert(dir.to_owned());
                    found = true;
                    break;
                }
            }
        }
        if !found {
            return Err(format!("No package for {}", input.display()).into());
        }
    }
    let mut out = OpenOptions::new().append(true).open(bundle)?;
    for dir in packages {
        let info: Value = serde_json::from_slice(&fs::read(dir.join("package.json"))?)?;
        let name = info["name"].as_str().ok_or("missing package name")?;
        let version = info["version"].as_str().ok_or("missing package version")?;
        let license = [
            "LICENSE",
            "LICENSE.md",
            "LICENSE.txt",
            "license",
            "license.md",
            "license.txt",
            "LICENSE-MIT",
            "COPYING",
        ]
        .into_iter()
        .map(|p| dir.join(p))
        .find(|p| p.is_file());
        let text = match license {
            Some(path) => fs::read_to_string(path)?,
            None => {
                let readme = fs::read_to_string(dir.join("README.md")).unwrap_or_default();
                let offset = readme
                    .match_indices("## License")
                    .find_map(|(i, h)| {
                        if i != 0 && !readme[..i].ends_with('\n') {
                            return None;
                        }
                        let rest = &readme[i + h.len()..];
                        let (tail, body) = rest.split_once('\n')?;
                        tail.trim()
                            .is_empty()
                            .then_some(body.trim_start_matches(['\n', '\r']))
                    })
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| format!("Missing bundled license: {name}"))?;
                offset.to_owned()
            }
        };
        writeln!(
            out,
            "\n/*! {name} {version}\n{} */",
            text.replace("*/", "* /")
        )?;
    }
    Ok(())
}

fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let web = root.join("../../src/issues/web");
    let stage = tempfile::Builder::new()
        .prefix(".web-build-")
        .tempdir_in(root)?;
    let output = stage.path();
    // npm installs the platform's native esbuild executable even with lifecycle scripts disabled.
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        s => s,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        "x86" => "ia32",
        s => s,
    };
    let esbuild = root.join(format!("node_modules/@esbuild/{os}-{arch}/bin/esbuild"));
    if !esbuild.is_file() {
        return Err(format!(
            "Missing {}. Run npm ci --ignore-scripts in {}.",
            esbuild.display(),
            root.display()
        )
        .into());
    }
    for entry in ["artifact-editor", "artifact-diagrams"] {
        let meta = output.join(format!("{entry}.json"));
        let mut c = Command::new(&esbuild);
        c.current_dir(root).arg(format!("{entry}.mjs")).args([
            "--bundle",
            "--minify",
            "--target=safari16,chrome100",
            "--legal-comments=eof",
            "--log-level=warning",
        ]);
        if entry == "artifact-diagrams" {
            c.args([
                "--format=esm",
                "--splitting",
                "--chunk-names=diagram-assets/[name]-[hash]",
            ]);
        } else {
            c.arg("--format=iife");
        }
        let status = c
            .arg(format!("--outdir={}", output.display()))
            .arg(format!("--metafile={}", meta.display()))
            .status()?;
        if !status.success() {
            return Err(format!("esbuild failed for {entry}: {status}").into());
        }
        append_licenses(root, &meta, &output.join(format!("{entry}.js")))?;
    }
    let chunks = Path::new("diagram-assets");
    let names = fs::read_dir(output.join(chunks))?
        .map(|e| e.map(|e| e.file_name()))
        .collect::<std::io::Result<BTreeSet<_>>>()?;
    let mut routes = String::from("// Generated by tools/web-assets.\n&[\n");
    for name in &names {
        let route = chunks.join(name);
        let route = route.to_str().ok_or("invalid chunk name")?;
        routes.push_str(&format!(
            "    (\"/{route}\", include_bytes!(\"{route}\")),\n"
        ));
    }
    routes.push_str("]\n");
    fs::write(output.join("diagram-assets.rs"), routes)?;
    // Finish both bundles and notices before touching the files embedded in the app.
    fs::create_dir_all(web.join(chunks))?;
    for file in names.iter().map(|name| chunks.join(name)).chain(
        [
            "artifact-editor.js",
            "artifact-diagrams.js",
            "diagram-assets.rs",
        ]
        .map(PathBuf::from),
    ) {
        fs::rename(output.join(&file), web.join(&file))?;
    }
    for file in fs::read_dir(web.join(chunks))? {
        let file = file?;
        if !names.contains(&file.file_name()) && file.file_type()?.is_file() {
            fs::remove_file(file.path())?
        }
    }
    println!(
        "Built editor, diagram loader, and {} diagram chunks.",
        names.len()
    );
    Ok(())
}
