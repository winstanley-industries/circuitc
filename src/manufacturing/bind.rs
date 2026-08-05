use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::design::{
    CopperLayer, Design, ManufacturabilityCapability, PopulationState, ProductVariant,
};
use crate::product::{ProductArtifactBundle, verify_product_artifact_bundle};
use crate::{CompiledArtifacts, RelativeArtifactPath};

use super::contract::{
    ArtifactBinding, DrillBinding, DrillProfile, ExportProfile, ExporterIdentity,
    FABRICATION_IDENTITY_DOMAIN, FabricationCompilerArtifacts, FabricationDiagnostic,
    FabricationFile, FabricationHostFile, FabricationIdentityPreimage, FabricationManifest,
    FabricationManifestBundle, FabricationRequest, FabricationRequestBundle, FileBinding,
    GerberBinding, GerberJobBinding, GerberLayerProfile, GerberProfile, KICAD_ADAPTER, KICAD_MAJOR,
    KICAD_VERSION, MANIFEST_SCHEMA, MAX_AGGREGATE_BYTES, MAX_FILE_BYTES, OutputDescriptor,
    PositionBinding, PositionProfile, REQUEST_SCHEMA, ResourcePolicy, SCHEMA_VERSION,
};
use super::normalize::{
    ExpectedPosition, normalize_excellon, normalize_gerber, normalize_gerber_job,
    parse_position_csv,
};

fn diagnostic(
    code: &'static str,
    path: impl Into<String>,
    message: impl Into<String>,
) -> FabricationDiagnostic {
    FabricationDiagnostic {
        code,
        path: path.into(),
        message: message.into(),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn canonical_json<T: Serialize>(value: &T, path: &str) -> Result<String, FabricationDiagnostic> {
    let mut rendered = serde_json::to_string(value).map_err(|error| {
        diagnostic(
            "CC-FABRICATION-CONTRACT-001",
            path,
            format!("fabrication contract serialization failed: {error}"),
        )
    })?;
    rendered.push('\n');
    if rendered.len() > MAX_FILE_BYTES {
        return Err(diagnostic(
            "CC-FABRICATION-RESOURCE-001",
            path,
            "fabrication contract exceeds the 64 MiB byte limit",
        ));
    }
    Ok(rendered)
}

fn artifact_path(path: String) -> Result<RelativeArtifactPath, FabricationDiagnostic> {
    RelativeArtifactPath::try_new(path)
        .map_err(|error| diagnostic("CC-FABRICATION-CONTRACT-001", "path", error.to_string()))
}

fn root_string(value: &Value, field: &str) -> Result<String, FabricationDiagnostic> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            diagnostic(
                "CC-FABRICATION-AUTH-001",
                field,
                "verified product artifact root field is unavailable",
            )
        })
}

#[derive(Clone)]
struct ProductRoots {
    variant_identity_sha256: String,
    product_input_sha256: String,
    product_resolution_sha256: String,
    placement_sha256: String,
    catalog_evaluated_on: String,
}

fn verify_product_roots(
    design: &Design,
    snapshot_bytes: &[u8],
    variant_path: &str,
    product: &ProductArtifactBundle,
) -> Result<ProductRoots, FabricationDiagnostic> {
    verify_product_artifact_bundle(design, snapshot_bytes, variant_path, product).map_err(
        |diagnostics| {
            let detail = diagnostics
                .first()
                .map(ToString::to_string)
                .unwrap_or_else(|| "product bundle verification failed".to_owned());
            diagnostic(
                "CC-FABRICATION-AUTH-001",
                "product",
                format!("Layer-3 product bundle is not authoritative: {detail}"),
            )
        },
    )?;
    let resolution: Value = serde_json::from_str(&product.resolution_json).map_err(|error| {
        diagnostic(
            "CC-FABRICATION-AUTH-001",
            "resolution.json",
            format!("verified product resolution could not be decoded: {error}"),
        )
    })?;
    let placement: Value = serde_json::from_str(&product.placement_json).map_err(|error| {
        diagnostic(
            "CC-FABRICATION-AUTH-001",
            "placement.json",
            format!("verified product placement could not be decoded: {error}"),
        )
    })?;
    let variant_identity_sha256 = root_string(&resolution, "variant_identity_sha256")?;
    let product_input_sha256 = root_string(&resolution, "product_input_sha256")?;
    let product_resolution_sha256 = sha256_hex(product.resolution_json.as_bytes());
    if root_string(&placement, "variant_identity_sha256")? != variant_identity_sha256
        || root_string(&placement, "product_input_sha256")? != product_input_sha256
        || root_string(&placement, "product_resolution_sha256")? != product_resolution_sha256
    {
        return Err(diagnostic(
            "CC-FABRICATION-AUTH-001",
            "product",
            "verified resolution and placement roots do not reconcile",
        ));
    }
    Ok(ProductRoots {
        variant_identity_sha256,
        product_input_sha256,
        product_resolution_sha256,
        placement_sha256: sha256_hex(product.placement_json.as_bytes()),
        catalog_evaluated_on: root_string(&resolution, "catalog_evaluated_on")?,
    })
}

fn select_analysis(
    design: &Design,
    analysis_path: &str,
    assertion_path: &str,
) -> Result<(), FabricationDiagnostic> {
    let analysis = design
        .product
        .manufacturability_analyses
        .iter()
        .find(|analysis| analysis.path == analysis_path)
        .ok_or_else(|| {
            diagnostic(
                "CC-FABRICATION-AUTH-001",
                "analysis_path",
                "Design does not declare the selected manufacturability analysis",
            )
        })?;
    if analysis.adapter != KICAD_ADAPTER || analysis.version != KICAD_MAJOR.to_string() {
        return Err(diagnostic(
            "CC-FABRICATION-AUTH-001",
            "analysis_path",
            "fabrication v1 requires the authored kicad major-version 10 adapter",
        ));
    }
    let assertion = analysis
        .assertions
        .iter()
        .find(|assertion| assertion.path == assertion_path)
        .ok_or_else(|| {
            diagnostic(
                "CC-FABRICATION-AUTH-001",
                "assertion_path",
                "selected analysis does not declare the fabrication assertion",
            )
        })?;
    if assertion.capability != ManufacturabilityCapability::FabricationInventoryComplete {
        return Err(diagnostic(
            "CC-FABRICATION-AUTH-001",
            "assertion_path",
            "selected assertion is not fabrication_inventory_complete",
        ));
    }
    Ok(())
}

fn fixed_layers(design_name: &str) -> Vec<GerberLayerProfile> {
    [
        (
            0,
            "F.Cu",
            "Copper,L1,Top",
            "Copper,L1,Top",
            "Positive",
            "F_Cu",
        ),
        (
            1,
            "F.Mask",
            "Soldermask,Top",
            "SolderMask,Top",
            "Negative",
            "F_Mask",
        ),
        (
            2,
            "B.Cu",
            "Copper,L2,Bot",
            "Copper,L2,Bot",
            "Positive",
            "B_Cu",
        ),
        (
            3,
            "B.Mask",
            "Soldermask,Bot",
            "SolderMask,Bot",
            "Negative",
            "B_Mask",
        ),
        (
            5,
            "F.SilkS",
            "Legend,Top",
            "Legend,Top",
            "Positive",
            "F_Silkscreen",
        ),
        (
            7,
            "B.SilkS",
            "Legend,Bot",
            "Legend,Bot",
            "Positive",
            "B_Silkscreen",
        ),
        (
            13,
            "F.Paste",
            "Paste,Top",
            "SolderPaste,Top",
            "Positive",
            "F_Paste",
        ),
        (
            15,
            "B.Paste",
            "Paste,Bot",
            "SolderPaste,Bot",
            "Positive",
            "B_Paste",
        ),
        (
            25,
            "Edge.Cuts",
            "Profile,NP",
            "Profile",
            "Positive",
            "Edge_Cuts",
        ),
    ]
    .into_iter()
    .map(
        |(layer_id, layer_name, function, job_function, polarity, filename_layer)| {
            GerberLayerProfile {
                layer_id,
                layer_name: layer_name.to_owned(),
                file_function: function.to_owned(),
                job_file_function: job_function.to_owned(),
                file_polarity: polarity.to_owned(),
                path: format!("gerber/{design_name}-{filename_layer}.gbr"),
            }
        },
    )
    .collect()
}

fn fixed_profile(design_name: &str) -> ExportProfile {
    ExportProfile {
        gerber: GerberProfile {
            format: "x2".to_owned(),
            precision: 6,
            net_attributes: true,
            protel_extensions: false,
            origin: "page".to_owned(),
            board_plot_params: false,
            layers: fixed_layers(design_name),
        },
        drill: DrillProfile {
            format: "excellon".to_owned(),
            origin: "absolute".to_owned(),
            units: "mm".to_owned(),
            zero_format: "decimal".to_owned(),
            oval_format: "alternate".to_owned(),
            mirror_y: false,
            minimal_header: false,
            separate_plated: true,
            generate_map: false,
            generate_report: false,
            generate_tenting: false,
        },
        position: PositionProfile {
            format: "csv".to_owned(),
            units: "mm".to_owned(),
            side: "both".to_owned(),
            origin: "page".to_owned(),
            bottom_negate_x: false,
            smd_only: false,
            exclude_through_hole: false,
            exclude_dnp: false,
            variant: None,
        },
        resources: ResourcePolicy::default(),
    }
}

fn fixed_outputs(design_name: &str, profile: &ExportProfile) -> Vec<OutputDescriptor> {
    let mut outputs: Vec<_> = profile
        .gerber
        .layers
        .iter()
        .map(|layer| OutputDescriptor {
            role: format!("gerber_layer_{}", layer.layer_id),
            path: layer.path.clone(),
        })
        .collect();
    outputs.extend([
        OutputDescriptor {
            role: "gerber_job".to_owned(),
            path: format!("gerber/{design_name}-job.gbrjob"),
        },
        OutputDescriptor {
            role: "drill_non_plated_through".to_owned(),
            path: format!("drill/{design_name}-NPTH.drl"),
        },
        OutputDescriptor {
            role: "drill_plated_through".to_owned(),
            path: format!("drill/{design_name}-PTH.drl"),
        },
        OutputDescriptor {
            role: "position_all".to_owned(),
            path: format!("position/{design_name}-all-pos.csv"),
        },
    ]);
    outputs
}

struct Prepared {
    request: FabricationRequest,
    request_json: String,
    request_path: RelativeArtifactPath,
    manifest_path: RelativeArtifactPath,
}

fn authenticate_compiler_artifacts<'a>(
    design: &Design,
    evidence: FabricationCompilerArtifacts<'a>,
) -> Result<&'a CompiledArtifacts, FabricationDiagnostic> {
    let requires_checked = !design.analyses.is_empty() || !design.board.routing_requests.is_empty();
    match evidence {
        FabricationCompilerArtifacts::Static(compiled) if !requires_checked => {
            let expected = crate::compile(design).map_err(|error| {
                let detail = error
                    .diagnostics
                    .first()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "Design compilation failed".to_owned());
                diagnostic(
                    "CC-FABRICATION-AUTH-001",
                    "design",
                    format!("Design cannot be compiled for fabrication: {detail}"),
                )
            })?;
            if *compiled != expected {
                return Err(diagnostic(
                    "CC-FABRICATION-AUTH-001",
                    format!("{}.compiled_artifacts", design.name),
                    "supplied static artifacts are stale or were not emitted by this compiler",
                ));
            }
            Ok(compiled)
        }
        FabricationCompilerArtifacts::Static(_) => Err(diagnostic(
            "CC-FABRICATION-AUTH-001",
            "compiled_artifacts",
            "simulation or routing intent requires opaque checked compiler artifacts",
        )),
        FabricationCompilerArtifacts::Checked(checked) if requires_checked => {
            crate::compile::authenticate_checked_compilation(design, checked).map_err(
                |diagnostics| {
                    let detail = diagnostics
                        .first()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "checked compilation authentication failed".to_owned());
                    diagnostic("CC-FABRICATION-AUTH-001", "compiled_artifacts", detail)
                },
            )
        }
        FabricationCompilerArtifacts::Checked(_) => Err(diagnostic(
            "CC-FABRICATION-AUTH-001",
            "compiled_artifacts",
            "static Design requires independently reproducible static compiler artifacts",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn prepare(
    design: &Design,
    snapshot_bytes: &[u8],
    variant_path: &str,
    compiler_artifacts: FabricationCompilerArtifacts<'_>,
    product: &ProductArtifactBundle,
    analysis_path: &str,
    assertion_path: &str,
) -> Result<Prepared, FabricationDiagnostic> {
    select_analysis(design, analysis_path, assertion_path)?;
    let compiled = authenticate_compiler_artifacts(design, compiler_artifacts)?;
    if compiled.kicad_pcb.len() > MAX_FILE_BYTES {
        return Err(diagnostic(
            "CC-FABRICATION-RESOURCE-001",
            format!("{}.kicad_pcb", design.name),
            "supplied KiCad board exceeds the 64 MiB byte limit",
        ));
    }
    let roots = verify_product_roots(design, snapshot_bytes, variant_path, product)?;
    let profile = fixed_profile(&design.name);
    let outputs = fixed_outputs(&design.name, &profile);
    let preimage = FabricationIdentityPreimage {
        design_name: design.name.clone(),
        analysis_path: analysis_path.to_owned(),
        assertion_path: assertion_path.to_owned(),
        variant_path: variant_path.to_owned(),
        variant_identity_sha256: roots.variant_identity_sha256.clone(),
        product_input_sha256: roots.product_input_sha256.clone(),
        product_resolution_sha256: roots.product_resolution_sha256.clone(),
        placement_sha256: roots.placement_sha256.clone(),
        catalog_evaluated_on: roots.catalog_evaluated_on.clone(),
        kicad_pcb: ArtifactBinding {
            path: format!("{}.kicad_pcb", design.name),
            sha256: sha256_hex(compiled.kicad_pcb.as_bytes()),
        },
        expected_adapter: KICAD_ADAPTER.to_owned(),
        expected_major: KICAD_MAJOR,
        expected_version: KICAD_VERSION.to_owned(),
        export_profile: profile.clone(),
        outputs: outputs.clone(),
    };
    let preimage_json = serde_json::to_vec(&preimage).map_err(|error| {
        diagnostic(
            "CC-FABRICATION-CONTRACT-001",
            "fabrication_identity_sha256",
            format!("fabrication identity serialization failed: {error}"),
        )
    })?;
    let mut hasher = Sha256::new();
    hasher.update(FABRICATION_IDENTITY_DOMAIN);
    hasher.update(preimage_json);
    let fabrication_identity_sha256: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let request = FabricationRequest {
        schema_name: REQUEST_SCHEMA.to_owned(),
        schema_version: SCHEMA_VERSION,
        design_name: preimage.design_name,
        analysis_path: preimage.analysis_path,
        assertion_path: preimage.assertion_path,
        variant_path: preimage.variant_path,
        variant_identity_sha256: preimage.variant_identity_sha256,
        product_input_sha256: preimage.product_input_sha256,
        product_resolution_sha256: preimage.product_resolution_sha256,
        placement_sha256: preimage.placement_sha256,
        catalog_evaluated_on: preimage.catalog_evaluated_on,
        kicad_pcb: preimage.kicad_pcb,
        expected_adapter: preimage.expected_adapter,
        expected_major: preimage.expected_major,
        expected_version: preimage.expected_version,
        fabrication_identity_sha256: fabrication_identity_sha256.clone(),
        export_profile: preimage.export_profile,
        outputs: preimage.outputs,
    };
    let root = format!("fabrication/{fabrication_identity_sha256}");
    let request_path = artifact_path(format!("{root}/request.json"))?;
    let manifest_path = artifact_path(format!("{root}/manifest.json"))?;
    let request_json = canonical_json(&request, request_path.as_str())?;
    Ok(Prepared {
        request,
        request_json,
        request_path,
        manifest_path,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn prepare_kicad10_fabrication_request(
    design: &Design,
    snapshot_bytes: &[u8],
    variant_path: &str,
    compiled: FabricationCompilerArtifacts<'_>,
    product: &ProductArtifactBundle,
    analysis_path: &str,
    assertion_path: &str,
) -> Result<FabricationRequestBundle, FabricationDiagnostic> {
    let prepared = prepare(
        design,
        snapshot_bytes,
        variant_path,
        compiled,
        product,
        analysis_path,
        assertion_path,
    )?;
    let expected_host_paths = prepared
        .request
        .outputs
        .iter()
        .map(|output| artifact_path(output.path.clone()))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(FabricationRequestBundle {
        fabrication_identity_sha256: prepared.request.fabrication_identity_sha256,
        request_path: prepared.request_path,
        manifest_path: prepared.manifest_path,
        request_json: prepared.request_json,
        expected_host_paths,
    })
}

fn expected_positions(
    design: &Design,
    variant_path: &str,
) -> Result<Vec<ExpectedPosition>, FabricationDiagnostic> {
    let variant: &ProductVariant = design
        .product
        .variants
        .iter()
        .find(|variant| variant.path == variant_path)
        .ok_or_else(|| {
            diagnostic(
                "CC-FABRICATION-AUTH-001",
                "variant_path",
                "Design does not declare the fabrication variant",
            )
        })?;
    let states: BTreeMap<_, _> = variant
        .components
        .iter()
        .map(|entry| (entry.component_path.as_str(), &entry.state))
        .collect();
    let mut positions = Vec::new();
    for component in design
        .components
        .iter()
        .filter(|component| component.physical.is_some())
    {
        let physical = component
            .physical
            .as_ref()
            .expect("filtered physical component");
        let state = match states.get(component.path.as_str()).copied() {
            Some(PopulationState::Fitted) => "fitted",
            Some(PopulationState::NotFitted) => "not_fitted",
            Some(PopulationState::Alternate(_)) => "alternate",
            None => {
                return Err(diagnostic(
                    "CC-FABRICATION-AUTH-001",
                    &component.path,
                    "variant population is not total over physical components",
                ));
            }
        };
        let host_package = physical
            .footprint
            .library_id
            .rsplit_once(':')
            .map(|(_, name)| name)
            .ok_or_else(|| {
                diagnostic(
                    "CC-FABRICATION-AUTH-001",
                    &component.path,
                    "physical footprint lacks a KiCad library nickname",
                )
            })?;
        positions.push(ExpectedPosition {
            component_path: component.path.clone(),
            reference: component.reference.clone(),
            host_value: component.value_label(),
            host_package: host_package.to_owned(),
            x_nm: physical.placement.position.x,
            y_nm: physical.placement.position.y,
            rotation_degrees: physical.placement.rotation_degrees.rem_euclid(360),
            side: match physical.placement.layer {
                CopperLayer::Front => "front".to_owned(),
                CopperLayer::Back => "back".to_owned(),
            },
            state: state.to_owned(),
        });
    }
    positions.sort_by(|left, right| left.component_path.cmp(&right.component_path));
    Ok(positions)
}

fn bind_file(path: &str, contents: &[u8]) -> Result<FileBinding, FabricationDiagnostic> {
    Ok(FileBinding {
        path: path.to_owned(),
        byte_length: u64::try_from(contents.len()).map_err(|_| {
            diagnostic(
                "CC-FABRICATION-RESOURCE-001",
                path,
                "fabrication file length does not fit u64",
            )
        })?,
        sha256: sha256_hex(contents),
    })
}

#[allow(clippy::too_many_arguments)]
pub fn bind_kicad10_fabrication(
    design: &Design,
    snapshot_bytes: &[u8],
    variant_path: &str,
    compiled: FabricationCompilerArtifacts<'_>,
    product: &ProductArtifactBundle,
    analysis_path: &str,
    assertion_path: &str,
    host_version: &str,
    host_executable: &[u8],
    host_files: &[FabricationHostFile],
) -> Result<FabricationManifestBundle, FabricationDiagnostic> {
    if host_version != KICAD_VERSION
        || host_executable.is_empty()
        || host_executable.len() > 512 * 1024 * 1024
    {
        return Err(diagnostic(
            "CC-FABRICATION-HOST-001",
            "exporter",
            "fabrication requires exact KiCad 10.0.5 and bounded non-empty executable bytes",
        ));
    }
    let prepared = prepare(
        design,
        snapshot_bytes,
        variant_path,
        compiled,
        product,
        analysis_path,
        assertion_path,
    )?;
    if host_files.len() != prepared.request.outputs.len() {
        return Err(diagnostic(
            "CC-FABRICATION-INVENTORY-001",
            "host_files",
            "host file count does not match the exact request inventory",
        ));
    }
    let aggregate = host_files.iter().try_fold(0_usize, |total, file| {
        if file.contents.len() > MAX_FILE_BYTES {
            return Err(diagnostic(
                "CC-FABRICATION-RESOURCE-001",
                file.path.as_str(),
                "host file exceeds the 64 MiB byte limit",
            ));
        }
        total.checked_add(file.contents.len()).ok_or_else(|| {
            diagnostic(
                "CC-FABRICATION-RESOURCE-001",
                "host_files",
                "host file aggregate size overflowed",
            )
        })
    })?;
    if aggregate > MAX_AGGREGATE_BYTES {
        return Err(diagnostic(
            "CC-FABRICATION-RESOURCE-001",
            "host_files",
            "host file aggregate exceeds the 256 MiB limit",
        ));
    }
    let expected_set: BTreeSet<_> = prepared
        .request
        .outputs
        .iter()
        .map(|output| output.path.as_str())
        .collect();
    let mut input = BTreeMap::new();
    for file in host_files {
        if !expected_set.contains(file.path.as_str()) {
            return Err(diagnostic(
                "CC-FABRICATION-INVENTORY-001",
                file.path.as_str(),
                "host returned an unrequested fabrication path",
            ));
        }
        if input
            .insert(file.path.as_str(), file.contents.as_slice())
            .is_some()
        {
            return Err(diagnostic(
                "CC-FABRICATION-INVENTORY-001",
                file.path.as_str(),
                "host returned a duplicate fabrication path",
            ));
        }
    }
    if input.len() != expected_set.len() {
        return Err(diagnostic(
            "CC-FABRICATION-INVENTORY-001",
            "host_files",
            "host omitted one or more requested fabrication paths",
        ));
    }

    let root = format!(
        "fabrication/{}",
        prepared.request.fabrication_identity_sha256
    );
    let mut files = Vec::new();
    let mut gerbers = Vec::new();
    for layer in &prepared.request.export_profile.gerber.layers {
        let raw = input
            .get(layer.path.as_str())
            .expect("exact inventory checked");
        let normalized = normalize_gerber(
            &layer.path,
            raw,
            &design.name,
            &prepared.request.catalog_evaluated_on,
            layer,
        )?;
        let final_path = format!("{root}/{}", layer.path);
        let binding = bind_file(&final_path, &normalized.contents)?;
        gerbers.push(GerberBinding {
            layer_id: layer.layer_id,
            layer_name: layer.layer_name.clone(),
            file_function: layer.file_function.clone(),
            path: binding.path.clone(),
            byte_length: binding.byte_length,
            sha256: binding.sha256.clone(),
        });
        files.push(FabricationFile {
            path: artifact_path(final_path)?,
            contents: normalized.contents,
        });
    }

    let job_suffix = format!("gerber/{}-job.gbrjob", design.name);
    let normalized_job = normalize_gerber_job(
        &job_suffix,
        input
            .get(job_suffix.as_str())
            .expect("exact inventory checked"),
        &design.name,
        &prepared.request.catalog_evaluated_on,
        &prepared.request.export_profile.gerber.layers,
    )?;
    let job_path = format!("{root}/{job_suffix}");
    let job_file_binding = bind_file(&job_path, &normalized_job.contents)?;
    let gerber_job = GerberJobBinding {
        path: job_file_binding.path.clone(),
        byte_length: job_file_binding.byte_length,
        sha256: job_file_binding.sha256.clone(),
        gerber_count: u32::try_from(gerbers.len()).expect("nine Gerbers fit u32"),
    };
    files.push(FabricationFile {
        path: artifact_path(job_path)?,
        contents: normalized_job.contents,
    });

    let drill_specs = [
        (
            "non_plated_through",
            format!("drill/{}-NPTH.drl", design.name),
            false,
        ),
        (
            "plated_through",
            format!("drill/{}-PTH.drl", design.name),
            true,
        ),
    ];
    let mut drills = Vec::new();
    for (kind, suffix, plated) in drill_specs {
        let normalized = normalize_excellon(
            &suffix,
            input.get(suffix.as_str()).expect("exact inventory checked"),
            &prepared.request.catalog_evaluated_on,
            plated,
        )?;
        let final_path = format!("{root}/{suffix}");
        let binding = bind_file(&final_path, &normalized.contents)?;
        drills.push(DrillBinding {
            kind: kind.to_owned(),
            path: binding.path.clone(),
            byte_length: binding.byte_length,
            sha256: binding.sha256.clone(),
            tool_count: 0,
            round_hit_count: 0,
            slot_hit_count: 0,
        });
        files.push(FabricationFile {
            path: artifact_path(final_path)?,
            contents: normalized.contents,
        });
    }

    let position_suffix = format!("position/{}-all-pos.csv", design.name);
    let position_contents = input
        .get(position_suffix.as_str())
        .expect("exact inventory checked");
    let position_rows = parse_position_csv(
        &position_suffix,
        position_contents,
        &expected_positions(design, variant_path)?,
    )?;
    let position_path = format!("{root}/{position_suffix}");
    let position_file_binding = bind_file(&position_path, position_contents)?;
    let position_csv = PositionBinding {
        path: position_file_binding.path.clone(),
        byte_length: position_file_binding.byte_length,
        sha256: position_file_binding.sha256.clone(),
        row_count: u64::try_from(position_rows.len()).expect("bounded row count fits u64"),
    };
    files.push(FabricationFile {
        path: artifact_path(position_path)?,
        contents: position_contents.to_vec(),
    });
    files.sort_by(|left, right| left.path.cmp(&right.path));

    let request_binding = bind_file(
        prepared.request_path.as_str(),
        prepared.request_json.as_bytes(),
    )?;
    let manifest = FabricationManifest {
        schema_name: MANIFEST_SCHEMA.to_owned(),
        schema_version: SCHEMA_VERSION,
        design_name: prepared.request.design_name.clone(),
        analysis_path: prepared.request.analysis_path.clone(),
        assertion_path: prepared.request.assertion_path.clone(),
        variant_path: prepared.request.variant_path.clone(),
        variant_identity_sha256: prepared.request.variant_identity_sha256.clone(),
        product_input_sha256: prepared.request.product_input_sha256.clone(),
        product_resolution_sha256: prepared.request.product_resolution_sha256.clone(),
        placement_sha256: prepared.request.placement_sha256.clone(),
        catalog_evaluated_on: prepared.request.catalog_evaluated_on.clone(),
        kicad_pcb: prepared.request.kicad_pcb.clone(),
        fabrication_identity_sha256: prepared.request.fabrication_identity_sha256.clone(),
        request: request_binding,
        exporter: ExporterIdentity {
            adapter: KICAD_ADAPTER.to_owned(),
            version: host_version.to_owned(),
            executable_sha256: sha256_hex(host_executable),
        },
        export_profile: prepared.request.export_profile.clone(),
        gerbers,
        gerber_job,
        drills,
        position_csv,
        position_rows,
    };
    let manifest_json = canonical_json(&manifest, prepared.manifest_path.as_str())?;
    let normalized_aggregate = files.iter().try_fold(
        prepared
            .request_json
            .len()
            .checked_add(manifest_json.len())
            .ok_or_else(|| {
                diagnostic(
                    "CC-FABRICATION-RESOURCE-001",
                    "bundle",
                    "fabrication bundle aggregate size overflowed",
                )
            })?,
        |total, file| {
            total.checked_add(file.contents.len()).ok_or_else(|| {
                diagnostic(
                    "CC-FABRICATION-RESOURCE-001",
                    "bundle",
                    "fabrication bundle aggregate size overflowed",
                )
            })
        },
    )?;
    if normalized_aggregate > MAX_AGGREGATE_BYTES {
        return Err(diagnostic(
            "CC-FABRICATION-RESOURCE-001",
            "bundle",
            "fabrication bundle aggregate exceeds the 256 MiB limit",
        ));
    }
    Ok(FabricationManifestBundle {
        fabrication_identity_sha256: prepared.request.fabrication_identity_sha256,
        request_path: prepared.request_path,
        manifest_path: prepared.manifest_path,
        request_json: prepared.request_json,
        manifest_json,
        files,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn verify_kicad10_fabrication_manifest(
    design: &Design,
    snapshot_bytes: &[u8],
    variant_path: &str,
    compiled: FabricationCompilerArtifacts<'_>,
    product: &ProductArtifactBundle,
    analysis_path: &str,
    assertion_path: &str,
    host_version: &str,
    host_executable: &[u8],
    host_files: &[FabricationHostFile],
    supplied: &FabricationManifestBundle,
) -> Result<(), FabricationDiagnostic> {
    let expected = bind_kicad10_fabrication(
        design,
        snapshot_bytes,
        variant_path,
        compiled,
        product,
        analysis_path,
        assertion_path,
        host_version,
        host_executable,
        host_files,
    )?;
    if supplied != &expected {
        return Err(diagnostic(
            "CC-FABRICATION-VERIFY-001",
            "manifest",
            "fabrication manifest or normalized file bytes do not match authoritative recomputation",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::demo::voltage_divider;
    use crate::design::{
        SimulationAnalysis, SimulationAnalysisKind, SimulationAssertion, SimulationSample,
    };
    use crate::manufacturing::contract::MAX_POSITION_ROWS;
    use crate::product::compile_product_artifacts;
    use crate::quantity::{Quantity, Unit};

    use super::*;

    const SNAPSHOT: &[u8] = include_bytes!("../../catalogs/reference-catalog.json");
    const HOST_EXECUTABLE: &[u8] = b"test-kicad-10.0.5-executable";
    const ANALYSIS: &str = "release.manufacturability";
    const ASSERTION: &str = "release.manufacturability.fabrication";

    fn raw_gerber(design_name: &str, layer: &GerberLayerProfile, second: u8) -> Vec<u8> {
        let polarity = if layer.layer_name == "Edge.Cuts" {
            String::new()
        } else {
            format!("%TF.FilePolarity,{}*%\n", layer.file_polarity)
        };
        format!(
            "%TF.GenerationSoftware,KiCad,Pcbnew,10.0.5*%\n%TF.CreationDate,2026-08-04T08:00:{second:02}-07:00*%\n%TF.ProjectId,{design_name},00000000-0000-0000-0000-000000000000,rev?*%\n%TF.SameCoordinates,Original*%\n%TF.FileFunction,{}*%\n{}%FSLAX46Y46*%\nG04 Gerber Fmt 4.6, Leading zero omitted, Abs format (unit mm)*\nG04 Created by KiCad (PCBNEW 10.0.5) date 2026-08-04 08:00:{second:02}*\n%MOMM*%\n%LPD*%\nG01*\nM02*\n",
            layer.file_function, polarity
        )
        .into_bytes()
    }

    fn raw_job(design_name: &str, layers: &[GerberLayerProfile], second: u8) -> Vec<u8> {
        let attributes: Vec<Value> = layers
            .iter()
            .map(|layer| {
                serde_json::json!({
                    "Path": layer.path.rsplit('/').next().unwrap(),
                    "FileFunction": layer.job_file_function,
                    "FilePolarity": layer.file_polarity,
                })
            })
            .collect();
        let value = serde_json::json!({
            "Header": {
                "GenerationSoftware": {
                    "Vendor": "KiCad",
                    "Application": "Pcbnew",
                    "Version": "10.0.5"
                },
                "CreationDate": format!("2026-08-04T08:00:{second:02}-07:00")
            },
            "GeneralSpecs": {"ProjectId": {"Name": design_name}},
            "FilesAttributes": attributes
        });
        let mut rendered = serde_json::to_string_pretty(&value).unwrap();
        rendered.push('\n');
        rendered.into_bytes()
    }

    fn raw_drill(plated: bool, second: u8) -> Vec<u8> {
        let function = if plated {
            "Plated,1,2,PTH"
        } else {
            "NonPlated,1,2,NPTH"
        };
        format!(
            "M48\n; DRILL file KiCad 10.0.5 date 2026-08-04T08:00:{second:02}\n; FORMAT={{-:-/ absolute / metric / decimal}}\n; #@! TF.CreationDate,2026-08-04T08:00:{second:02}-07:00\n; #@! TF.GenerationSoftware,Kicad,Pcbnew,10.0.5\n; #@! TF.FileFunction,{function}\nFMAT,2\nMETRIC\n%\nG90\nG05\nM30\n"
        )
        .into_bytes()
    }

    fn raw_files(design: &Design, second: u8) -> Vec<FabricationHostFile> {
        let profile = fixed_profile(&design.name);
        let mut files: Vec<_> = profile
            .gerber
            .layers
            .iter()
            .map(|layer| FabricationHostFile {
                path: artifact_path(layer.path.clone()).unwrap(),
                contents: raw_gerber(&design.name, layer, second),
            })
            .collect();
        files.extend([
            FabricationHostFile {
                path: artifact_path(format!("gerber/{}-job.gbrjob", design.name)).unwrap(),
                contents: raw_job(&design.name, &profile.gerber.layers, second),
            },
            FabricationHostFile {
                path: artifact_path(format!("drill/{}-NPTH.drl", design.name)).unwrap(),
                contents: raw_drill(false, second),
            },
            FabricationHostFile {
                path: artifact_path(format!("drill/{}-PTH.drl", design.name)).unwrap(),
                contents: raw_drill(true, second),
            },
            FabricationHostFile {
                path: artifact_path(format!("position/{}-all-pos.csv", design.name)).unwrap(),
                contents: b"Ref,Val,Package,PosX,PosY,Rot,Side\n\"R1\",\"10k\xce\xa9\",\"R_0603_1608Metric\",15.000000,-10.000000,0.000000,top\n\"R2\",\"10k\xce\xa9\",\"R_0603_1608Metric\",25.000000,-10.000000,0.000000,top\n".to_vec(),
            },
        ]);
        files
    }

    fn bind_variant(variant: &str, second: u8) -> FabricationManifestBundle {
        let design = voltage_divider();
        let compiled = crate::compile(&design).unwrap();
        let product = compile_product_artifacts(&design, SNAPSHOT, variant).unwrap();
        bind_kicad10_fabrication(
            &design,
            SNAPSHOT,
            variant,
            FabricationCompilerArtifacts::Static(&compiled),
            &product,
            ANALYSIS,
            ASSERTION,
            KICAD_VERSION,
            HOST_EXECUTABLE,
            &raw_files(&design, second),
        )
        .unwrap()
    }

    fn bind_error_code(design: &Design, host_files: &[FabricationHostFile]) -> &'static str {
        let compiled = crate::compile(design).unwrap();
        let product = compile_product_artifacts(design, SNAPSHOT, "production").unwrap();
        bind_kicad10_fabrication(
            design,
            SNAPSHOT,
            "production",
            FabricationCompilerArtifacts::Static(&compiled),
            &product,
            ANALYSIS,
            ASSERTION,
            KICAD_VERSION,
            HOST_EXECUTABLE,
            host_files,
        )
        .unwrap_err()
        .code
    }

    #[test]
    fn fabrication_bundle_normalizes_only_host_clock_fields() {
        assert_eq!(
            bind_variant("production", 1),
            bind_variant("production", 59)
        );
    }

    #[test]
    fn static_compiler_evidence_cannot_bypass_checked_design_authority() {
        let mut design = voltage_divider();
        let stale_static = crate::compile(&design).unwrap();
        design.analyses.push(SimulationAnalysis {
            path: "simulation.dc".to_owned(),
            kind: SimulationAnalysisKind::DcOperatingPoint,
        });
        design.canonicalize();
        let product = compile_product_artifacts(&design, SNAPSHOT, "production").unwrap();
        assert_eq!(
            bind_kicad10_fabrication(
                &design,
                SNAPSHOT,
                "production",
                FabricationCompilerArtifacts::Static(&stale_static),
                &product,
                ANALYSIS,
                ASSERTION,
                KICAD_VERSION,
                HOST_EXECUTABLE,
                &raw_files(&design, 1),
            )
            .unwrap_err()
            .code,
            "CC-FABRICATION-AUTH-001"
        );
    }

    #[test]
    fn checked_compiler_evidence_authenticates_simulation_design_for_fabrication() {
        let mut design = voltage_divider();
        design.analyses.push(SimulationAnalysis {
            path: "simulation.dc".to_owned(),
            kind: SimulationAnalysisKind::DcOperatingPoint,
        });
        design.canonicalize();
        let work_root = std::env::var_os("TEST_TMPDIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("fabrication-checked-simulation-positive");
        let checked = crate::compile_checked(&design, &work_root).unwrap();
        let product = compile_product_artifacts(&design, SNAPSHOT, "production").unwrap();

        bind_kicad10_fabrication(
            &design,
            SNAPSHOT,
            "production",
            FabricationCompilerArtifacts::Checked(&checked),
            &product,
            ANALYSIS,
            ASSERTION,
            KICAD_VERSION,
            HOST_EXECUTABLE,
            &raw_files(&design, 1),
        )
        .expect("authenticated checked evidence must bind for a simulation Design");
    }

    #[test]
    fn fabrication_rejects_same_path_checked_simulation_semantic_drift() {
        let mut design = voltage_divider();
        design.analyses.push(SimulationAnalysis {
            path: "simulation.dc".to_owned(),
            kind: SimulationAnalysisKind::DcOperatingPoint,
        });
        design.canonicalize();
        let work_root = std::env::var_os("TEST_TMPDIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("fabrication-checked-simulation-stale");
        let checked = crate::compile_checked(&design, &work_root).unwrap();

        design.assertions.push(SimulationAssertion {
            path: "checks.dc".to_owned(),
            analysis_path: "simulation.dc".to_owned(),
            net: "VOUT".to_owned(),
            sample: SimulationSample::Scalar,
            expected: Quantity::new(5, 0, Unit::Volt),
            absolute_tolerance: Quantity::new(0, 0, Unit::Volt),
            relative_tolerance: Quantity::new(0, 0, Unit::Dimensionless),
        });
        design.canonicalize();
        let product = compile_product_artifacts(&design, SNAPSHOT, "production").unwrap();
        let diagnostic = bind_kicad10_fabrication(
            &design,
            SNAPSHOT,
            "production",
            FabricationCompilerArtifacts::Checked(&checked),
            &product,
            ANALYSIS,
            ASSERTION,
            KICAD_VERSION,
            HOST_EXECUTABLE,
            &raw_files(&design, 1),
        )
        .expect_err("fabrication must reject stale same-path checked simulation evidence");
        assert_eq!(diagnostic.code, "CC-FABRICATION-AUTH-001");
        assert_eq!(diagnostic.path, "compiled_artifacts");
        assert!(
            diagnostic
                .message
                .contains("checked simulation inputs do not equal deterministic lowering")
        );
    }

    #[test]
    fn checked_compiler_evidence_cannot_replace_static_recompilation() {
        let design = voltage_divider();
        let work_root = std::env::var_os("TEST_TMPDIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("fabrication-checked-static-negative");
        let checked = crate::compile_checked(&design, &work_root).unwrap();
        let product = compile_product_artifacts(&design, SNAPSHOT, "production").unwrap();

        let diagnostic = bind_kicad10_fabrication(
            &design,
            SNAPSHOT,
            "production",
            FabricationCompilerArtifacts::Checked(&checked),
            &product,
            ANALYSIS,
            ASSERTION,
            KICAD_VERSION,
            HOST_EXECUTABLE,
            &raw_files(&design, 1),
        )
        .expect_err("static Design must reject opaque checked compiler artifacts");
        assert_eq!(diagnostic.code, "CC-FABRICATION-AUTH-001");
        assert_eq!(diagnostic.path, "compiled_artifacts");
        assert_eq!(
            diagnostic.message,
            "static Design requires independently reproducible static compiler artifacts"
        );
    }

    #[test]
    fn all_footprint_position_parity_preserves_variant_population() {
        let bundle = bind_variant("prototype_alternate", 1);
        let manifest: Value = serde_json::from_str(&bundle.manifest_json).unwrap();
        let rows = manifest
            .get("position_rows")
            .and_then(Value::as_array)
            .unwrap();
        let states: BTreeMap<_, _> = rows
            .iter()
            .map(|row| {
                (
                    row.get("component_path").and_then(Value::as_str).unwrap(),
                    row.get("state").and_then(Value::as_str).unwrap(),
                )
            })
            .collect();
        assert_eq!(states.get("divider.r_top").copied(), Some("alternate"),);
        assert_eq!(states.get("divider.r_bottom").copied(), Some("not_fitted"),);
    }

    #[test]
    fn gerber_function_and_position_membership_mutants_fail_closed() {
        let design = voltage_divider();
        let compiled = crate::compile(&design).unwrap();
        let product = compile_product_artifacts(&design, SNAPSHOT, "production").unwrap();
        let mut files = raw_files(&design, 1);
        let gerber = files
            .iter_mut()
            .find(|file| file.path.as_str().ends_with("F_Cu.gbr"))
            .unwrap();
        gerber.contents = String::from_utf8(gerber.contents.clone())
            .unwrap()
            .replace("Copper,L1,Top", "Copper,L2,Bot")
            .into_bytes();
        assert_eq!(
            bind_kicad10_fabrication(
                &design,
                SNAPSHOT,
                "production",
                FabricationCompilerArtifacts::Static(&compiled),
                &product,
                ANALYSIS,
                ASSERTION,
                KICAD_VERSION,
                HOST_EXECUTABLE,
                &files,
            )
            .unwrap_err()
            .code,
            "CC-FABRICATION-GERBER-001"
        );

        let mut files = raw_files(&design, 1);
        files
            .iter_mut()
            .find(|file| file.path.as_str().ends_with("all-pos.csv"))
            .unwrap()
            .contents
            .extend_from_slice(
                b"\"R3\",\"10k\xce\xa9\",\"R_0603_1608Metric\",1.000000,1.000000,0.000000,top\n",
            );
        assert_eq!(
            bind_kicad10_fabrication(
                &design,
                SNAPSHOT,
                "production",
                FabricationCompilerArtifacts::Static(&compiled),
                &product,
                ANALYSIS,
                ASSERTION,
                KICAD_VERSION,
                HOST_EXECUTABLE,
                &files,
            )
            .unwrap_err()
            .code,
            "CC-FABRICATION-POSITION-001"
        );
    }

    #[test]
    fn controlled_native_metadata_mutants_fail_closed() {
        let design = voltage_divider();
        for extra in [
            "%TF.FileFunction,Copper,L2,Bot*%\n",
            "%TF.FilePolarity,Negative*%\n",
            "%FSTAX25Y25*%\n",
            "%MOIN*%\n",
            "%TF.CreationDate,malformed\n",
            "G04 Created by KiCad (PCBNEW 10.0.5) date malformed\n",
        ] {
            let mut files = raw_files(&design, 1);
            let gerber = files
                .iter_mut()
                .find(|file| file.path.as_str().ends_with("F_Cu.gbr"))
                .unwrap();
            let text = String::from_utf8(gerber.contents.clone()).unwrap();
            gerber.contents = text
                .replace("M02*\n", &format!("{extra}M02*\n"))
                .into_bytes();
            assert_eq!(
                bind_error_code(&design, &files),
                "CC-FABRICATION-GERBER-001"
            );
        }

        for mutation in ["duplicate_timestamp", "malformed_timestamp"] {
            let mut files = raw_files(&design, 1);
            let gerber = files
                .iter_mut()
                .find(|file| file.path.as_str().ends_with("F_Cu.gbr"))
                .unwrap();
            let text = String::from_utf8(gerber.contents.clone()).unwrap();
            gerber.contents = if mutation == "duplicate_timestamp" {
                text.replace(
                    "%TF.CreationDate,2026-08-04T08:00:01-07:00*%\n",
                    "%TF.CreationDate,2026-08-04T08:00:01-07:00*%\n%TF.CreationDate,2026-08-04T08:00:02-07:00*%\n",
                )
                .into_bytes()
            } else {
                text.replace("2026-08-04T08:00:01-07:00", "not-a-timestamp")
                    .into_bytes()
            };
            assert_eq!(
                bind_error_code(&design, &files),
                "CC-FABRICATION-GERBER-001"
            );
        }

        let mut files = raw_files(&design, 1);
        let job = files
            .iter_mut()
            .find(|file| file.path.as_str().ends_with("job.gbrjob"))
            .unwrap();
        job.contents = String::from_utf8(job.contents.clone())
            .unwrap()
            .replace(
                "\"CreationDate\": \"2026-08-04T08:00:01-07:00\"",
                "\"CreationDate\": \"2026-08-04T08:00:01-07:00\",\n    \"CreationDate\": \"2026-08-04T08:00:01-07:00\"",
            )
            .into_bytes();
        assert_eq!(
            bind_error_code(&design, &files),
            "CC-FABRICATION-GERBER-001"
        );

        let mut files = raw_files(&design, 1);
        let job = files
            .iter_mut()
            .find(|file| file.path.as_str().ends_with("job.gbrjob"))
            .unwrap();
        let text = String::from_utf8(job.contents.clone()).unwrap();
        job.contents = format!(
            "{},\n  \"Injected\":{{\"CreationDate\":\"2026-08-04T08:00:59-07:00\"}}\n}}\n",
            text.strip_suffix("}\n").unwrap()
        )
        .into_bytes();
        assert_eq!(
            bind_error_code(&design, &files),
            "CC-FABRICATION-GERBER-001"
        );
    }

    #[test]
    fn relocated_concatenated_and_reordered_native_records_fail_closed() {
        let design = voltage_divider();
        let creation = "%TF.CreationDate,2026-08-04T08:00:01-07:00*%\n";
        let created = "G04 Created by KiCad (PCBNEW 10.0.5) date 2026-08-04 08:00:01*\n";
        for controlled in [creation, created] {
            let mut files = raw_files(&design, 1);
            let gerber = files
                .iter_mut()
                .find(|file| file.path.as_str().ends_with("F_Cu.gbr"))
                .unwrap();
            let text = String::from_utf8(gerber.contents.clone()).unwrap();
            gerber.contents = text
                .replacen(controlled, "", 1)
                .replace("M02*\n", &format!("{controlled}M02*\n"))
                .into_bytes();
            assert_eq!(
                bind_error_code(&design, &files),
                "CC-FABRICATION-GERBER-001"
            );
        }

        let mut files = raw_files(&design, 1);
        let gerber = files
            .iter_mut()
            .find(|file| file.path.as_str().ends_with("F_Cu.gbr"))
            .unwrap();
        gerber.contents = String::from_utf8(gerber.contents.clone())
            .unwrap()
            .replace("rev?*%\n", "rev?*%%TF.FileFunction,Copper,L2,Bot*%\n")
            .into_bytes();
        assert_eq!(
            bind_error_code(&design, &files),
            "CC-FABRICATION-GERBER-001"
        );

        for mutation in ["reordered", "relocated", "mismatched_timestamp"] {
            let mut files = raw_files(&design, 1);
            let drill = files
                .iter_mut()
                .find(|file| file.path.as_str().ends_with("PTH.drl"))
                .unwrap();
            let text = String::from_utf8(drill.contents.clone()).unwrap();
            drill.contents = match mutation {
                "reordered" => text.replace("G90\nG05\n", "G05\nG90\n").into_bytes(),
                "relocated" => text
                    .replacen("; #@! TF.CreationDate,2026-08-04T08:00:01-07:00\n", "", 1)
                    .replace(
                        "%\n",
                        "%\n; #@! TF.CreationDate,2026-08-04T08:00:01-07:00\n",
                    )
                    .into_bytes(),
                _ => text
                    .replace(
                        "; #@! TF.CreationDate,2026-08-04T08:00:01-07:00",
                        "; #@! TF.CreationDate,2026-08-04T08:00:02-07:00",
                    )
                    .into_bytes(),
            };
            assert_eq!(bind_error_code(&design, &files), "CC-FABRICATION-DRILL-001");
        }
    }

    #[test]
    fn inventory_drill_and_position_mutants_fail_closed() {
        let design = voltage_divider();

        let mut missing = raw_files(&design, 1);
        missing.pop();
        assert_eq!(
            bind_error_code(&design, &missing),
            "CC-FABRICATION-INVENTORY-001"
        );

        let mut duplicate = raw_files(&design, 1);
        duplicate[1] = duplicate[0].clone();
        assert_eq!(
            bind_error_code(&design, &duplicate),
            "CC-FABRICATION-INVENTORY-001"
        );

        let mut extra = raw_files(&design, 1);
        extra[0].path = artifact_path("gerber/unrequested.gbr".to_owned()).unwrap();
        assert_eq!(
            bind_error_code(&design, &extra),
            "CC-FABRICATION-INVENTORY-001"
        );

        let mut drill_hit = raw_files(&design, 1);
        let drill = drill_hit
            .iter_mut()
            .find(|file| file.path.as_str().ends_with("PTH.drl"))
            .unwrap();
        drill.contents = String::from_utf8(drill.contents.clone())
            .unwrap()
            .replace("M30\n", "T1C0.300\nX100000Y100000\nM30\n")
            .into_bytes();
        assert_eq!(
            bind_error_code(&design, &drill_hit),
            "CC-FABRICATION-DRILL-001"
        );

        for (from, to) in [
            ("15.000000", "15.000001"),
            ("0.000000,top", "90.000000,top"),
            ("0.000000,top", "0.000000,bottom"),
        ] {
            let mut files = raw_files(&design, 1);
            let position = files
                .iter_mut()
                .find(|file| file.path.as_str().ends_with("all-pos.csv"))
                .unwrap();
            position.contents = String::from_utf8(position.contents.clone())
                .unwrap()
                .replacen(from, to, 1)
                .into_bytes();
            assert_eq!(
                bind_error_code(&design, &files),
                "CC-FABRICATION-POSITION-001"
            );
        }

        let mut duplicate_position = raw_files(&design, 1);
        let position = duplicate_position
            .iter_mut()
            .find(|file| file.path.as_str().ends_with("all-pos.csv"))
            .unwrap();
        position.contents = String::from_utf8(position.contents.clone())
            .unwrap()
            .replace("\"R2\"", "\"R1\"")
            .into_bytes();
        assert_eq!(
            bind_error_code(&design, &duplicate_position),
            "CC-FABRICATION-POSITION-001"
        );
    }

    #[test]
    fn resource_boundary_and_executable_identity_are_bound() {
        let design = voltage_divider();
        let compiled = crate::compile(&design).unwrap();
        let product = compile_product_artifacts(&design, SNAPSHOT, "production").unwrap();
        let files = raw_files(&design, 1);
        let first = bind_kicad10_fabrication(
            &design,
            SNAPSHOT,
            "production",
            FabricationCompilerArtifacts::Static(&compiled),
            &product,
            ANALYSIS,
            ASSERTION,
            KICAD_VERSION,
            b"host executable A",
            &files,
        )
        .unwrap();
        let second = bind_kicad10_fabrication(
            &design,
            SNAPSHOT,
            "production",
            FabricationCompilerArtifacts::Static(&compiled),
            &product,
            ANALYSIS,
            ASSERTION,
            KICAD_VERSION,
            b"host executable B",
            &files,
        )
        .unwrap();
        assert_ne!(first.manifest_json, second.manifest_json);
        assert_eq!(
            verify_kicad10_fabrication_manifest(
                &design,
                SNAPSHOT,
                "production",
                FabricationCompilerArtifacts::Static(&compiled),
                &product,
                ANALYSIS,
                ASSERTION,
                KICAD_VERSION,
                b"host executable B",
                &files,
                &first,
            )
            .unwrap_err()
            .code,
            "CC-FABRICATION-VERIFY-001"
        );

        {
            let mut exact = raw_files(&design, 1);
            exact[0].contents = vec![b'\n'; MAX_FILE_BYTES];
            assert_eq!(
                bind_error_code(&design, &exact),
                "CC-FABRICATION-GERBER-001"
            );
            exact[0].contents.push(b'\n');
            assert_eq!(
                bind_error_code(&design, &exact),
                "CC-FABRICATION-RESOURCE-001"
            );
        }
        let mut newline_dense_drill = raw_files(&design, 1);
        newline_dense_drill
            .iter_mut()
            .find(|file| file.path.as_str().ends_with("PTH.drl"))
            .unwrap()
            .contents = vec![b'\n'; MAX_FILE_BYTES];
        assert_eq!(
            bind_error_code(&design, &newline_dense_drill),
            "CC-FABRICATION-DRILL-001"
        );
    }

    #[test]
    fn position_row_limit_is_inclusive() {
        let mut expected = Vec::with_capacity(MAX_POSITION_ROWS);
        let mut csv = String::from("Ref,Val,Package,PosX,PosY,Rot,Side\n");
        for index in 0..MAX_POSITION_ROWS {
            expected.push(ExpectedPosition {
                component_path: format!("component.{index:05}"),
                reference: format!("R{index}"),
                host_value: "v".to_owned(),
                host_package: "p".to_owned(),
                x_nm: i64::try_from(index).unwrap() * 1_000_000,
                y_nm: 0,
                rotation_degrees: 0,
                side: "front".to_owned(),
                state: "fitted".to_owned(),
            });
            csv.push_str(&format!(
                "R{index},v,p,{index}.000000,0.000000,0.000000,top\n"
            ));
        }
        assert_eq!(
            parse_position_csv("position.csv", csv.as_bytes(), &expected)
                .unwrap()
                .len(),
            MAX_POSITION_ROWS
        );
        csv.push_str("R10000,v,p,10000.000000,0.000000,0.000000,top\n");
        assert_eq!(
            parse_position_csv("position.csv", csv.as_bytes(), &expected)
                .unwrap_err()
                .code,
            "CC-FABRICATION-RESOURCE-001"
        );
    }

    #[test]
    fn verifier_rejects_coordinated_manifest_and_file_rewrites() {
        let design = voltage_divider();
        let compiled = crate::compile(&design).unwrap();
        let product = compile_product_artifacts(&design, SNAPSHOT, "production").unwrap();
        let files = raw_files(&design, 1);
        let mut bundle = bind_kicad10_fabrication(
            &design,
            SNAPSHOT,
            "production",
            FabricationCompilerArtifacts::Static(&compiled),
            &product,
            ANALYSIS,
            ASSERTION,
            KICAD_VERSION,
            HOST_EXECUTABLE,
            &files,
        )
        .unwrap();
        bundle.files[0].contents.push(b'\n');
        bundle.manifest_json = bundle
            .manifest_json
            .replace("\"byte_length\":", "\"byte_length\":0,\"forged\":");
        assert_eq!(
            verify_kicad10_fabrication_manifest(
                &design,
                SNAPSHOT,
                "production",
                FabricationCompilerArtifacts::Static(&compiled),
                &product,
                ANALYSIS,
                ASSERTION,
                KICAD_VERSION,
                HOST_EXECUTABLE,
                &files,
                &bundle,
            )
            .unwrap_err()
            .code,
            "CC-FABRICATION-VERIFY-001"
        );
    }
}
