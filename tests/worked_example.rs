//! End-to-end engine tests against realistic project sources, modeled on
//! the `examples/sample-kmp` worked example in the Uliab repository.

use lsp_types::Url;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use ulb_lsp::diagnostics::{DiagnosticEngine, SourceLoader};
use ulb_lsp::document::Document;

struct MapLoader(HashMap<PathBuf, String>);

impl SourceLoader for MapLoader {
    fn load(&self, path: &Path) -> Option<String> {
        self.0.get(path).cloned()
    }
}

const CONVENTIONS: &str = r#"
convention androidApp {
  android {
    compileSdk 37
    minSdk 24
    targetSdk 37
  }
  buildTypes {
    release { minifyEnabled true }
  }
}

convention envSigning {
  signing {
    storeFile     props("signing.properties").storeFile
    storePassword env("STORE_PASSWORD")
  }
}

fn defaultDebug() {
  buildTypes { debug { minifyEnabled false } }
}
"#;

const LIBS: &str = r#"
versions {
  coreVersion = "1.15.0"
  composeVersion = "1.8.0"
}

appcompat = "androidx.appcompat:appcompat:1.7.0"
coreKtx   = "androidx.core:core-ktx" @ coreVersion
ui        = "org.jetbrains.compose.ui:ui" @ composeVersion
kotlinxCoroutines = "org.jetbrains.kotlinx:kotlinx-coroutines-core" @ "1.9.0"

bundle {
  ui = [ ui, appcompat ]
}

plugins {
  android = "ulite/android" @ "0.3.0"
  kmp     = "ulite/kmp"     @ "0.3.0"
}
"#;

const BUILD: &str = r#"
plugin "android"
plugin "kmp"

apply "androidApp"
apply "envSigning"

android {
  namespace "com.example.app"
  applicationId "com.example.app"
  versionCode 7
  versionName ver(major=0, minor=1, patch=2)
}

buildTypes {
  release {
    proguardFiles [ "proguard-rules.pro" ]
  }
}

productFlavors {
  dimension "tier"
  free { applicationIdSuffix ".free" }
  paid { applicationIdSuffix ".paid" }
}

signing {
  keyAlias    props("signing.properties").keyAlias
  keyPassword env("KEY_PASSWORD")
}

deps {
  implementation "androidx.core:core-ktx" @ coreVersion
  implementation appcompat
}

commonMain.deps {
  implementation kotlinxCoroutines
}
androidMain.deps {
  implementation "org.jetbrains.compose.ui:ui" @ composeVersion
}

defaultDebug()

task "printConfig" {
  description "Prints the resolved module configuration."
  dependsOn [ "compileReleaseKotlin", "bundleRelease" ]
  run {
    exec(command="echo", args=["hello", "from", "ulb"])
    copy(from="src/main/kotlin", to="out/merged-kotlin")
  }
}
"#;

fn url(path: &str) -> Url {
    Url::from_file_path(path).expect("absolute file path")
}

const SETTINGS: &str = r#"
project "SampleKmp"
module "."
"#;

fn loader_with_project() -> MapLoader {
    let mut map = HashMap::new();
    map.insert(
        PathBuf::from("/proj/conventions.ulb"),
        CONVENTIONS.to_owned(),
    );
    map.insert(PathBuf::from("/proj/libs.ulb"), LIBS.to_owned());
    map.insert(PathBuf::from("/proj/settings.ulb"), SETTINGS.to_owned());
    MapLoader(map)
}

#[test]
fn worked_example_project_reports_no_diagnostics() {
    let mut engine = DiagnosticEngine::with_loader(loader_with_project());
    let build = url("/proj/build.ulb");
    engine.upsert(build.clone(), Document::new(BUILD.to_owned(), 1));
    let diagnostics = engine.diagnostics_for(&build);
    assert!(
        diagnostics.is_empty(),
        "expected no diagnostics, got: {diagnostics:?}"
    );
}

#[test]
fn worked_example_plus_unknown_apply_flags_one_error() {
    let mut engine = DiagnosticEngine::with_loader(loader_with_project());
    let build = url("/proj/build.ulb");
    engine.upsert(
        build.clone(),
        Document::new(format!("{BUILD}\napply \"nonexistent\"\n"), 1),
    );
    let diagnostics = engine.diagnostics_for(&build);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].message, "unknown convention 'nonexistent'");
    let range = diagnostics[0].range;
    assert_eq!(range.start.line, 54);
    assert_eq!(range.end.line, 54);
}

#[test]
fn mid_edit_source_still_produces_parse_diagnostics() {
    let mut engine = DiagnosticEngine::new();
    let build = url("/proj/build.ulb");
    engine.upsert(
        build.clone(),
        Document::new(
            "apply \"androidApp\"\nandroid {\n  compileSdk \n}\n".to_owned(),
            1,
        ),
    );
    let diagnostics = engine.diagnostics_for(&build);
    assert!(
        !diagnostics.is_empty(),
        "a missing value after the key must surface a parse error"
    );
    assert!(
        diagnostics
            .iter()
            .all(|d| d.severity == Some(lsp_types::DiagnosticSeverity::ERROR))
    );
}

#[test]
fn worked_example_settings_file_is_clean() {
    let mut engine = DiagnosticEngine::with_loader(loader_with_project());
    let settings = url("/proj/settings.ulb");
    engine.upsert(settings.clone(), Document::new(SETTINGS.to_owned(), 1));
    let diagnostics = engine.diagnostics_for(&settings);
    assert!(
        diagnostics.is_empty(),
        "expected no diagnostics, got: {diagnostics:?}"
    );
}

#[test]
fn settings_unknown_key_surfaces_semantic_error() {
    let mut engine = DiagnosticEngine::new();
    let settings = url("/proj/settings.ulb");
    engine.upsert(
        settings.clone(),
        Document::new(format!("{SETTINGS}\nmodl \"typo\"\n"), 1),
    );
    let diagnostics = engine.diagnostics_for(&settings);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].range.start.line, 3);
}
