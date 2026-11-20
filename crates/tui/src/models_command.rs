//! `socium models import`: writes `models.d/` files from the models.dev
//! catalog, or lists its providers.

use crate::cli::ModelsImport;
use socium_core::model_import::{self, Catalog, ProviderSummary};
use std::io::Write;
use std::path::Path;
use std::time::SystemTime;

pub(crate) async fn main(command: ModelsImport) -> i32 {
    let (text, source) = match &command.from {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(text) => (text, path.clone()),
            Err(err) => {
                eprintln!("socium: cannot read {path}: {err}");
                return 1;
            }
        },
        None => match model_import::fetch_catalog().await {
            Ok(text) => (text, model_import::CATALOG_URL.to_string()),
            Err(err) => {
                eprintln!("socium: {err}");
                return 1;
            }
        },
    };
    let catalog = match model_import::parse_catalog(&text) {
        Ok(catalog) => catalog,
        Err(err) => {
            eprintln!("socium: {source}: {err}");
            return 1;
        }
    };
    let mut stdout = std::io::stdout().lock();
    let mut stderr = std::io::stderr().lock();
    if command.providers.is_empty() {
        list(&model_import::summarize(&catalog), &mut stdout);
        return 0;
    }
    let Some(dir) = model_import::models_dir() else {
        eprintln!("socium: set HOME or XDG_CONFIG_HOME so the files have a place to go");
        return 1;
    };
    let date = model_import::utc_date(SystemTime::now());
    let request = Request {
        catalog: &catalog,
        source: &source,
        date: &date,
        dir: &dir,
    };
    run(&request, &command.providers, &mut stdout, &mut stderr)
}

struct Request<'a> {
    catalog: &'a Catalog,
    source: &'a str,
    date: &'a str,
    dir: &'a Path,
}

/// Imports each provider, reporting what was written to `out` and what was
/// left out to `err`. Returns the exit status: 1 when any provider failed.
fn run(request: &Request, providers: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let mut status = 0;
    for id in providers {
        let import = match model_import::import(request.catalog, id, request.source, request.date) {
            Ok(import) => import,
            Err(reason) => {
                let _ = writeln!(err, "socium: skipped provider {id}: {reason}");
                status = 1;
                continue;
            }
        };
        let path = match model_import::write(request.dir, &import) {
            Ok(path) => path,
            Err(error) => {
                let _ = writeln!(
                    err,
                    "socium: cannot write {}: {error}",
                    request.dir.join(format!("{id}.toml")).display()
                );
                status = 1;
                continue;
            }
        };
        let models = if import.models == 1 {
            "model"
        } else {
            "models"
        };
        let _ = writeln!(out, "{}: {} {models}", path.display(), import.models);
        for endpoint in &import.endpoints {
            let _ = writeln!(
                out,
                "  provider {} ({}) sends its key to {}",
                endpoint.name, endpoint.wire, endpoint.base_url
            );
        }
        for line in &import.skipped {
            let _ = writeln!(err, "  skipped {line}");
        }
    }
    status
}

fn list(summary: &[ProviderSummary], out: &mut dyn Write) {
    let width = summary.iter().map(|line| line.id.len()).max().unwrap_or(0);
    for line in summary {
        let status = match &line.importable {
            Ok(models) => format!("imports {models} of {}", line.catalog_models),
            Err(reason) => format!("cannot import: {reason}"),
        };
        let _ = writeln!(out, "{:<width$}  {status}", line.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CATALOG: &str = r#"{
        "local": {"npm": "@ai-sdk/openai-compatible", "api": "https://local.example/v1", "env": ["LOCAL_KEY"], "models": {
            "small": {"id": "small", "name": "Small"},
            "old": {"id": "old", "status": "deprecated"}
        }},
        "bedrock": {"npm": "@ai-sdk/amazon-bedrock", "env": ["AWS_ACCESS_KEY_ID"], "models": {"m": {"id": "m"}}}
    }"#;

    fn run_import(dir: &Path, providers: &[&str]) -> (i32, String, String) {
        let catalog = model_import::parse_catalog(CATALOG).unwrap();
        let request = Request {
            catalog: &catalog,
            source: "saved.json",
            date: "2026-11-20",
            dir,
        };
        let providers = providers
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let status = run(&request, &providers, &mut out, &mut err);
        (
            status,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn import_writes_one_file_per_provider_and_reports_where_keys_go() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models.d");
        let (status, out, err) = run_import(&models_dir, &["local", "bedrock"]);
        assert_eq!(status, 1);
        let path = models_dir.join("local.toml");
        assert_eq!(
            out,
            format!(
                "{}: 1 model\n  provider local (openai-chat) sends its key to https://local.example/v1\n",
                path.display()
            )
        );
        assert!(err.contains("  skipped local/old: deprecated\n"), "{err}");
        assert!(
            err.contains("socium: skipped provider bedrock: no model can be imported"),
            "{err}"
        );
        let first = std::fs::read_to_string(&path).unwrap();
        assert!(first.contains("from saved.json on 2026-11-20"));
        assert!(!models_dir.join("bedrock.toml").exists());

        let (status, _, _) = run_import(&models_dir, &["local"]);
        assert_eq!(status, 0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
    }

    #[test]
    fn without_providers_it_lists_what_maps() {
        let catalog = model_import::parse_catalog(CATALOG).unwrap();
        let mut out = Vec::new();
        list(&model_import::summarize(&catalog), &mut out);
        let out = String::from_utf8(out).unwrap();
        let lines = out.lines().collect::<Vec<_>>();
        assert!(lines[0].starts_with("bedrock  cannot import: "), "{out}");
        assert_eq!(lines[1], "local    imports 1 of 2");
    }
}
