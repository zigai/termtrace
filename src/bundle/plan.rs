use super::*;

/// Return the deterministic content and sensitivity policy without writing.
#[must_use]
pub fn plan(profile: BundleProfile) -> BundlePlan {
    let mut members = vec![
        planned(
            "screen.txt",
            ArtifactSensitivity::SensitiveTerminalData,
            "selected_screen",
        ),
        planned(
            "observation.json",
            ArtifactSensitivity::SensitiveTerminalData,
            "selected_observation",
        ),
    ];
    if matches!(profile, BundleProfile::Debug | BundleProfile::Forensic) {
        members.push(planned(
            "verification.json",
            ArtifactSensitivity::OperationalMetadata,
            "source_verification",
        ));
    }
    if profile == BundleProfile::Debug {
        members.push(planned(
            "events-metadata.jsonl",
            ArtifactSensitivity::OperationalMetadata,
            "metadata_only_event_projection",
        ));
    }
    if profile == BundleProfile::Forensic {
        members.push(planned(
            "canonical/",
            ArtifactSensitivity::SensitiveTerminalData,
            "complete_top_level_canonical_payload",
        ));
    }
    return BundlePlan {
        format: "termtrace-bundle-plan".to_owned(),
        version: 1,
        profile,
        includes_canonical_evidence: profile == BundleProfile::Forensic,
        members,
    };
}

pub(super) fn planned(
    path: &str,
    sensitivity: ArtifactSensitivity,
    relationship: &str,
) -> BundlePlanMember {
    return BundlePlanMember {
        path: path.to_owned(),
        sensitivity,
        relationship: relationship.to_owned(),
    };
}

#[derive(Clone, Copy)]
pub(super) struct BundleMemberSemantics {
    pub(super) sensitivity: ArtifactSensitivity,
    pub(super) relationship: &'static str,
    pub(super) schema_type: Option<&'static str>,
}

pub(super) fn bundle_member_semantics(
    profile: BundleProfile,
    path: &str,
) -> Option<BundleMemberSemantics> {
    let sensitive = ArtifactSensitivity::SensitiveTerminalData;
    let operational = ArtifactSensitivity::OperationalMetadata;
    return match path {
        "screen.txt" => Some(BundleMemberSemantics {
            sensitivity: sensitive,
            relationship: "selected_screen",
            schema_type: None,
        }),
        "observation.json" => Some(BundleMemberSemantics {
            sensitivity: sensitive,
            relationship: "selected_observation",
            schema_type: Some("observation.schema.json"),
        }),
        "verification.json"
            if matches!(profile, BundleProfile::Debug | BundleProfile::Forensic) =>
        {
            Some(BundleMemberSemantics {
                sensitivity: operational,
                relationship: "source_verification",
                schema_type: Some("verification-report.schema.json"),
            })
        }
        "events-metadata.jsonl" if profile == BundleProfile::Debug => Some(BundleMemberSemantics {
            sensitivity: operational,
            relationship: "metadata_only_event_projection",
            schema_type: Some("event-projection.schema.json"),
        }),
        value if profile == BundleProfile::Forensic && canonical_member_name(value).is_some() => {
            Some(BundleMemberSemantics {
                sensitivity: sensitive,
                relationship: "canonical_evidence",
                schema_type: canonical_schema_type(value),
            })
        }
        _ => None,
    };
}

pub(super) fn canonical_member_name(path: &str) -> Option<&str> {
    let name = path.strip_prefix("canonical/")?;
    return (!name.is_empty() && !name.contains('/')).then_some(name);
}

pub(super) fn canonical_schema_type(path: &str) -> Option<&'static str> {
    return match canonical_member_name(path)? {
        "session.json" => Some("session.schema.json"),
        "completion.json" => Some("completion.schema.json"),
        "artifact-roots.jsonl" => Some("artifact-root-record.schema.json"),
        _ => None,
    };
}
