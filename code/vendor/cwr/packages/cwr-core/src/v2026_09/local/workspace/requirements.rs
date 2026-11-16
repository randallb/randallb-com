#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceReadiness {
    Ready,
    Partial,
    NeedsSetup,
    Invalid,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfiguredRuntimeReadiness {
    pub name: String,
    pub readiness: WorkspaceReadiness,
    pub issues: Vec<RequirementIssue>,
}

/// The same value-free report is used by automation and attended hosts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceRequirements {
    pub readiness: WorkspaceReadiness,
    pub requirements: Vec<Requirement>,
    #[serde(alias = "loops")]
    pub runtimes: Vec<ConfiguredRuntimeReadiness>,
    pub issues: Vec<RequirementIssue>,
}

impl LocalWorkspaceComposition {
    pub fn requirements(&self) -> CwrResult<WorkspaceRequirements> {
        self.requirements_with_environment(&BTreeSet::new())
    }

    /// Availability comes from the host. Inspection never reads environment values.
    pub fn requirements_with_environment(
        &self,
        available: &BTreeSet<String>,
    ) -> CwrResult<WorkspaceRequirements> {
        let (_, root) = requirements::workspace_input_contract(
            &self.path,
            &self.inputs,
            host_bindings(&self.bindings, available, true)?,
        )?;
        let mut report = WorkspaceRequirements {
            readiness: WorkspaceReadiness::Ready,
            requirements: root.requirements,
            runtimes: Vec::new(),
            issues: root.issues,
        };
        for volume in &self.volumes {
            report.requirements.push(Requirement {
                scope: self.path.to_string_lossy().into_owned(),
                alias: volume.name.clone(),
                type_ref: "volume".into(),
                purpose: volume.purpose.clone(),
                required: volume.required,
                status: if volume.path.is_dir() {
                    RequirementStatus::Supplied
                } else {
                    RequirementStatus::Missing
                },
                sources: Vec::new(),
                runtime_name: None,
                requested_access: None,
                available_access: Some(access_name(volume.access).into()),
            });
        }
        for configured in &self.runtimes {
            let mut inspected = InputInspection::default();
            let inputs = configured.select_inputs(available, Some(&mut inspected)).unwrap_or_else(|error| {
                inspected.issues.push(RequirementIssue {
                    scope: configured.cwr_toml.to_string_lossy().into_owned(),
                    status: RequirementStatus::Invalid,
                    message: error.to_string(),
                });
                InputBindings::new()
            });
            let result = configured.graph_options_with_imported_volumes().and_then(|options| {
                crate::v2026_09::graph::inspect_bound_graph(
                    &configured.cwr_toml,
                    &CompositionResourceResolver,
                    &options,
                    inputs,
                )
            });
            match result {
                Ok((mut result, graph)) => {
                    if let Some(graph) = graph {
                        let catalog = configured.acting_actor_ref(&graph).and_then(|actor| {
                            local_catalog(&runtime_graph(graph, &configured.cwr_toml)?, &actor)
                        });
                        if catalog.is_err() {
                            result.issues.push(RequirementIssue {
                                scope: configured.cwr_toml.to_string_lossy().into_owned(),
                                status: RequirementStatus::Invalid,
                                message: "configured runtime does not pass startup validation".into(),
                            });
                        }
                    }
                    inspected.requirements.extend(result.requirements);
                    inspected.issues.extend(result.issues);
                },
                Err(error) => inspected.issues.push(RequirementIssue {
                    scope: configured.cwr_toml.to_string_lossy().into_owned(),
                    status: RequirementStatus::Invalid,
                    message: error.to_string(),
                }),
            }
            for row in &mut inspected.requirements {
                row.runtime_name = Some(configured.name.clone());
            }
            let (volumes, issues) = configured.volume_requirements()?;
            inspected.requirements.extend(volumes);
            inspected.issues.extend(issues);
            let readiness = input_readiness(&inspected);
            report.requirements.extend(inspected.requirements);
            report.runtimes.push(ConfiguredRuntimeReadiness {
                name: configured.name.clone(),
                readiness,
                issues: inspected.issues,
            });
        }
        let ready = report
            .runtimes
            .iter()
            .filter(|row| row.readiness == WorkspaceReadiness::Ready)
            .count();
        report.readiness = if report
            .issues
            .iter()
            .any(|issue| issue.status == RequirementStatus::Invalid)
            || report.runtimes.iter().any(|row| row.readiness == WorkspaceReadiness::Invalid)
        {
            WorkspaceReadiness::Invalid
        } else if report
            .issues
            .iter()
            .any(|issue| issue.status == RequirementStatus::Unsupported)
            || report.runtimes.iter().any(|row| row.readiness == WorkspaceReadiness::Unsupported)
        {
            WorkspaceReadiness::Unsupported
        } else if ready == report.runtimes.len()
            && report
                .requirements
                .iter()
                .filter(|row| row.runtime_name.is_none())
                .all(|row| !row.required || row.status == RequirementStatus::Supplied)
        {
            WorkspaceReadiness::Ready
        } else if ready > 0 {
            WorkspaceReadiness::Partial
        } else {
            WorkspaceReadiness::NeedsSetup
        };
        Ok(report)
    }
}

fn input_readiness(report: &InputInspection) -> WorkspaceReadiness {
    if report
        .issues
        .iter()
        .any(|issue| issue.status == RequirementStatus::Invalid)
        || report
            .requirements
            .iter()
            .any(|row| row.status == RequirementStatus::Invalid)
    {
        WorkspaceReadiness::Invalid
    } else if report
        .issues
        .iter()
        .any(|issue| issue.status == RequirementStatus::Unsupported)
        || report
            .requirements
            .iter()
            .any(|row| row.status == RequirementStatus::Unsupported)
    {
        WorkspaceReadiness::Unsupported
    } else if report
        .requirements
        .iter()
        .any(|row| row.required && row.status == RequirementStatus::Missing)
        || report
            .issues
            .iter()
            .any(|issue| issue.status == RequirementStatus::Missing)
    {
        WorkspaceReadiness::NeedsSetup
    } else {
        WorkspaceReadiness::Ready
    }
}

impl std::fmt::Display for WorkspaceRequirements {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Workspace: {}", readiness_name(self.readiness))?;
        for requirement in self
            .requirements
            .iter()
            .filter(|item| item.runtime_name.is_none())
        {
            writeln!(
                f,
                "  {} ({}, {}): {}",
                requirement.alias,
                requirement
                    .type_ref
                    .rsplit('/')
                    .next()
                    .unwrap_or(&requirement.type_ref),
                if requirement.required {
                    "required"
                } else {
                    "optional"
                },
                status_name(requirement.status)
            )?;
            if !requirement.purpose.is_empty() {
                writeln!(f, "    {}", requirement.purpose)?;
            }
        }
        for row in &self.runtimes {
            writeln!(f, "{}: {}", row.name, readiness_name(row.readiness))?;
            for requirement in self
                .requirements
                .iter()
                .filter(|item| item.runtime_name.as_deref() == Some(&row.name))
            {
                writeln!(
                    f,
                    "  {} ({}, {}): {}",
                    requirement.alias,
                    requirement
                        .type_ref
                        .rsplit('/')
                        .next()
                        .unwrap_or(&requirement.type_ref),
                    if requirement.required {
                        "required"
                    } else {
                        "optional"
                    },
                    status_name(requirement.status)
                )?;
                if !requirement.purpose.is_empty() {
                    writeln!(f, "    {}", requirement.purpose)?;
                }
                for source in &requirement.sources {
                    writeln!(
                        f,
                        "    from {} / {}",
                        Path::new(&source.scope)
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy(),
                        source.alias
                    )?;
                }
                if let Some(access) = &requirement.requested_access {
                    writeln!(
                        f,
                        "    access: {access}; available: {}",
                        requirement.available_access.as_deref().unwrap_or("none")
                    )?;
                }
            }
            for issue in &row.issues {
                writeln!(f, "  {}", issue.message)?;
            }
        }
        for issue in &self.issues {
            writeln!(f, "{}", issue.message)?;
        }
        Ok(())
    }
}
fn readiness_name(readiness: WorkspaceReadiness) -> &'static str {
    match readiness {
        WorkspaceReadiness::Ready => "ready",
        WorkspaceReadiness::Partial => "partially configured",
        WorkspaceReadiness::NeedsSetup => "needs setup",
        WorkspaceReadiness::Invalid => "invalid configuration",
        WorkspaceReadiness::Unsupported => "requirements cannot be inspected",
    }
}
fn status_name(status: RequirementStatus) -> &'static str {
    match status {
        RequirementStatus::Supplied => "supplied",
        RequirementStatus::Missing => "missing",
        RequirementStatus::Invalid => "invalid",
        RequirementStatus::Unsupported => "unsupported",
    }
}
