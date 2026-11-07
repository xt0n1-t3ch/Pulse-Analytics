//! An SPDX 2.3 JSON SBOM for the Windows installer, built from `cargo metadata`, and its
//! validator.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;

use crate::clock;
use crate::fsx;
use crate::platform_assets::Platform;

#[derive(Serialize)]
struct Checksum {
    algorithm: &'static str,
    #[serde(rename = "checksumValue")]
    checksum_value: String,
}

#[derive(Serialize)]
struct ExternalRef {
    #[serde(rename = "referenceCategory")]
    category: &'static str,
    #[serde(rename = "referenceType")]
    kind: &'static str,
    #[serde(rename = "referenceLocator")]
    locator: String,
}

#[derive(Serialize)]
struct Package {
    #[serde(rename = "SPDXID")]
    id: String,
    name: String,
    #[serde(rename = "versionInfo")]
    version_info: String,
    #[serde(rename = "downloadLocation")]
    download_location: String,
    #[serde(rename = "filesAnalyzed")]
    files_analyzed: bool,
    #[serde(rename = "licenseConcluded")]
    license_concluded: &'static str,
    #[serde(rename = "licenseDeclared")]
    license_declared: String,
    #[serde(rename = "copyrightText")]
    copyright_text: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    checksums: Option<Vec<Checksum>>,
    #[serde(rename = "externalRefs", skip_serializing_if = "Option::is_none")]
    external_refs: Option<Vec<ExternalRef>>,
}

#[derive(Serialize)]
struct SpdxFile {
    #[serde(rename = "SPDXID")]
    id: &'static str,
    #[serde(rename = "fileName")]
    file_name: String,
    checksums: Vec<Checksum>,
    #[serde(rename = "licenseConcluded")]
    license_concluded: &'static str,
    #[serde(rename = "copyrightText")]
    copyright_text: &'static str,
}

#[derive(Serialize)]
struct Relationship {
    #[serde(rename = "spdxElementId")]
    element: String,
    #[serde(rename = "relationshipType")]
    kind: &'static str,
    #[serde(rename = "relatedSpdxElement")]
    related: String,
}

#[derive(Serialize)]
struct CreationInfo {
    created: String,
    creators: Vec<&'static str>,
    #[serde(rename = "licenseListVersion")]
    license_list_version: &'static str,
}

#[derive(Serialize)]
struct Document {
    #[serde(rename = "spdxVersion")]
    spdx_version: &'static str,
    #[serde(rename = "dataLicense")]
    data_license: &'static str,
    #[serde(rename = "SPDXID")]
    id: &'static str,
    name: String,
    #[serde(rename = "documentNamespace")]
    document_namespace: String,
    #[serde(rename = "creationInfo")]
    creation_info: CreationInfo,
    packages: Vec<Package>,
    files: Vec<SpdxFile>,
    relationships: Vec<Relationship>,
}

/// SPDX identifiers allow letters, digits, `.` and `-` only.
fn spdx_safe(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Percent-encodes everything except RFC 3986 unreserved characters, like `Uri.EscapeDataString`.
fn escape_data_string(text: &str) -> String {
    let mut escaped = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            escaped.push(char::from(byte));
        } else {
            escaped.push_str(&format!("%{byte:02X}"));
        }
    }
    escaped
}

fn cargo_metadata(root: &Path) -> Result<Value> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args([
            "--locked",
            "metadata",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(root.join("Cargo.toml"))
        .output()
        .context("failed to start cargo metadata")?;
    if !output.status.success() {
        bail!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    serde_json::from_slice(&output.stdout).context("parse cargo metadata output")
}

fn text_of(package: &Value, key: &str) -> String {
    package
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// Builds the SPDX document from `cargo metadata` output; separate from the process
/// call so the tests feed it a fixed metadata value.
fn build_document(
    metadata: &Value,
    artifact_name: &str,
    artifact_hash: &str,
    package_name: &str,
    package_version: &str,
    created: String,
) -> Document {
    let root_id = format!("SPDXRef-Package-{}", spdx_safe(package_name));
    let checksum = || Checksum {
        algorithm: "SHA256",
        checksum_value: artifact_hash.to_owned(),
    };
    let mut packages = vec![Package {
        id: root_id.clone(),
        name: package_name.to_owned(),
        version_info: package_version.to_owned(),
        download_location: "NOASSERTION".to_owned(),
        files_analyzed: true,
        license_concluded: "NOASSERTION",
        license_declared: "NOASSERTION".to_owned(),
        copyright_text: "NOASSERTION",
        checksums: Some(vec![checksum()]),
        external_refs: None,
    }];
    let mut relationships = vec![Relationship {
        element: "SPDXRef-DOCUMENT".to_owned(),
        kind: "DESCRIBES",
        related: root_id.clone(),
    }];

    let mut dependencies: Vec<&Value> = metadata
        .get("packages")
        .and_then(Value::as_array)
        .map(|packages| packages.iter().collect())
        .unwrap_or_default();
    dependencies.sort_by_key(|p| {
        (
            text_of(p, "name"),
            text_of(p, "version"),
            text_of(p, "source"),
        )
    });

    let mut index = 0usize;
    for dependency in dependencies {
        let name = text_of(dependency, "name");
        let version = text_of(dependency, "version");
        if name == package_name && version == package_version {
            continue;
        }
        index += 1;
        let id = format!("SPDXRef-Package-{index}-{}", spdx_safe(&name));
        let license = text_of(dependency, "license");
        let source = text_of(dependency, "source");
        packages.push(Package {
            id: id.clone(),
            download_location: if source.trim().is_empty() {
                "NOASSERTION".to_owned()
            } else {
                source
            },
            license_declared: if license.trim().is_empty() {
                "NOASSERTION".to_owned()
            } else {
                license
            },
            external_refs: Some(vec![ExternalRef {
                category: "PACKAGE-MANAGER",
                kind: "purl",
                locator: format!(
                    "pkg:cargo/{}@{}",
                    escape_data_string(&name),
                    escape_data_string(&version)
                ),
            }]),
            version_info: version,
            name,
            files_analyzed: false,
            license_concluded: "NOASSERTION",
            copyright_text: "NOASSERTION",
            checksums: None,
        });
        relationships.push(Relationship {
            element: root_id.clone(),
            kind: "DEPENDS_ON",
            related: id,
        });
    }
    relationships.push(Relationship {
        element: root_id,
        kind: "CONTAINS",
        related: "SPDXRef-File-WindowsArtifact".to_owned(),
    });

    Document {
        spdx_version: "SPDX-2.3",
        data_license: "CC0-1.0",
        id: "SPDXRef-DOCUMENT",
        name: format!("{package_name}-{package_version}-windows-x64"),
        document_namespace: format!(
            "https://github.com/xt0n1-t3ch/{package_name}/sbom/{package_version}/{artifact_hash}"
        ),
        creation_info: CreationInfo {
            created,
            creators: vec!["Tool: Pulse release-contract SPDX generator/1"],
            license_list_version: "3.26",
        },
        packages,
        files: vec![SpdxFile {
            id: "SPDXRef-File-WindowsArtifact",
            file_name: artifact_name.to_owned(),
            checksums: vec![checksum()],
            license_concluded: "NOASSERTION",
            copyright_text: "NOASSERTION",
        }],
        relationships,
    }
}

/// Writes the SBOM for `artifact` to `output` using the dependency graph of `root`.
pub fn generate(
    artifact: &Path,
    output: &Path,
    package_name: &str,
    package_version: &str,
    root: &Path,
) -> Result<String> {
    if !artifact.is_file() {
        bail!("Artifact not found: {}", artifact.display());
    }
    let metadata = cargo_metadata(&fsx::absolute(root)?)?;
    let hash = fsx::sha256_file(artifact)?;
    let document = build_document(
        &metadata,
        &fsx::file_name(artifact),
        &hash,
        package_name,
        package_version,
        clock::utc_timestamp(clock::now_unix()),
    );
    let count = document.packages.len();
    let output = fsx::absolute(output)?;
    let mut text = serde_json::to_string_pretty(&document)?;
    text.push('\n');
    fsx::write_text(&output, &text)?;
    Ok(format!(
        "SPDX SBOM generated: {} packages={count} sha256={hash}",
        output.display()
    ))
}

/// Validates an SBOM written by [`generate`] against its artifact.
pub fn verify(
    artifact: &Path,
    sbom_path: &Path,
    package_name: &str,
    package_version: &str,
) -> Result<String> {
    if !artifact.is_file() {
        bail!("Artifact not found: {}", artifact.display());
    }
    let sbom: Value = serde_json::from_str(&fsx::read_text(sbom_path)?)
        .with_context(|| format!("parse {}", sbom_path.display()))?;
    let text = |value: &Value, key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    if text(&sbom, "spdxVersion").as_deref() != Some("SPDX-2.3")
        || text(&sbom, "dataLicense").as_deref() != Some("CC0-1.0")
    {
        bail!("SBOM must be an SPDX 2.3 JSON document under CC0-1.0");
    }
    let packages = sbom
        .get("packages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let roots = packages
        .iter()
        .filter(|p| {
            text(p, "name").as_deref() == Some(package_name)
                && text(p, "versionInfo").as_deref() == Some(package_version)
        })
        .count();
    if roots != 1 {
        bail!("SBOM must contain exactly one root package {package_name}@{package_version}");
    }

    let artifact_name = fsx::file_name(artifact);
    let files: Vec<&Value> = sbom
        .get("files")
        .and_then(Value::as_array)
        .map(|files| {
            files
                .iter()
                .filter(|f| text(f, "fileName").as_deref() == Some(artifact_name.as_str()))
                .collect()
        })
        .unwrap_or_default();
    if files.len() != 1 {
        bail!("SBOM does not identify the Windows artifact");
    }
    let expected = fsx::sha256_file(artifact)?;
    let actual: Vec<String> = files[0]
        .get("checksums")
        .and_then(Value::as_array)
        .map(|sums| {
            sums.iter()
                .filter(|sum| {
                    text(sum, "algorithm").is_some_and(|a| a.eq_ignore_ascii_case("SHA256"))
                })
                .filter_map(|sum| text(sum, "checksumValue"))
                .map(|value| value.to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default();
    if actual.len() != 1 || actual[0] != expected {
        bail!("SBOM Windows artifact checksum mismatch");
    }
    if packages.len() < 2 {
        bail!("SBOM dependency inventory is empty");
    }
    Ok(format!(
        "SPDX SBOM verified: package={package_name} version={package_version} sha256={expected}"
    ))
}

/// CI step: finds the single installer `.exe` in `dist`, then generates and verifies its SBOM
/// as `dist/pulse-<platform>.spdx.json`. The version comes from the root `package.json`.
pub fn generate_and_verify(dist: &Path, platform: Platform, root: &Path) -> Result<String> {
    if !matches!(platform, Platform::WindowsX64 | Platform::WindowsArm64) {
        bail!(
            "Windows SBOMs exist only for Windows platforms, got {}",
            platform.as_str()
        );
    }
    let executables: Vec<_> = fsx::list_files_sorted(dist)?
        .into_iter()
        .filter(|path| fsx::file_name(path).to_ascii_lowercase().ends_with(".exe"))
        .collect();
    if executables.len() != 1 {
        bail!(
            "Expected exactly one Windows executable, found {}",
            executables.len()
        );
    }
    let package: Value = serde_json::from_str(&fsx::read_text(&root.join("package.json"))?)
        .context("parse package.json")?;
    let version = package
        .get("version")
        .and_then(Value::as_str)
        .context("package.json has no version")?;
    let sbom = dist.join(format!("pulse-{}.spdx.json", platform.as_str()));
    let generated = generate(&executables[0], &sbom, "pulse", version, root)?;
    let verified = verify(&executables[0], &sbom, "pulse", version)?;
    Ok(format!("{generated}\n{verified}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn spdx_ids_and_purls_follow_the_old_generator_escaping() {
        assert_eq!(spdx_safe("tauri_plugin+x"), "tauri-plugin-x");
        assert_eq!(escape_data_string("1.0.0+meta"), "1.0.0%2Bmeta");
        assert_eq!(escape_data_string("a-b_c.d~e"), "a-b_c.d~e");
    }

    #[test]
    fn document_lists_the_root_first_then_sorted_dependencies() {
        let metadata = json!({"packages": [
            {"name": "zeta", "version": "2.0.0", "license": "MIT", "source": "registry+https://github.com/rust-lang/crates.io-index"},
            {"name": "pulse", "version": "1.7.2", "license": null, "source": null},
            {"name": "alpha", "version": "1.0.0", "license": null, "source": null},
        ]});
        let document = build_document(
            &metadata,
            "Setup.exe",
            "abc",
            "pulse",
            "1.7.2",
            "now".into(),
        );
        let names: Vec<_> = document.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["pulse", "alpha", "zeta"]);
        assert_eq!(document.packages[1].id, "SPDXRef-Package-1-alpha");
        assert_eq!(document.packages[1].license_declared, "NOASSERTION");
        assert_eq!(document.packages[1].download_location, "NOASSERTION");
        assert_eq!(document.packages[2].license_declared, "MIT");
        assert_eq!(document.relationships.len(), 4);
        assert_eq!(document.name, "pulse-1.7.2-windows-x64");
        let text = serde_json::to_string(&document).expect("json");
        assert!(text.starts_with(
            r#"{"spdxVersion":"SPDX-2.3","dataLicense":"CC0-1.0","SPDXID":"SPDXRef-DOCUMENT""#
        ));
    }
}
