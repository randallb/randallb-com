use super::*;

impl Loader<'_> {
    pub(super) fn resolve_pending(&mut self) -> CwrResult<()> {
        while !self.pending.is_empty() {
            self.refresh_exports()?;
            let pending = std::mem::take(&mut self.pending);
            let mut unresolved = Vec::new();
            let mut progressed = false;
            for (owner, locator, visibility) in pending {
                let loaded = match self.load_locator(owner, &locator, visibility) {
                    Ok(target) => target,
                    Err(error)
                        if self.allow_partial
                            && owner != 0
                            && !is_fatal_load_error(error.as_ref()) =>
                    {
                        self.warn(self.resources[owner].source.clone(), "A configured dependency is unavailable; other bindings can still be used.");
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                if loaded.is_some() {
                    progressed = true;
                } else {
                    unresolved.push((owner, locator, visibility));
                }
            }
            if !progressed {
                if self.allow_partial {
                    for (owner, _, _) in unresolved {
                        self.warn(
                            self.resources[owner].source.clone(),
                            "An imported resource could not be found.",
                        );
                    }
                    break;
                }
                let (owner, locator, _) = &unresolved[0];
                return invalid(format!(
                    "{}: unresolved resource ref {}",
                    self.resources[*owner].source.display(),
                    locator.reference.as_deref().unwrap_or_default()
                ));
            }
            self.pending.extend(unresolved);
        }
        for owner in 0..self.resources.len() {
            if self.disabled.contains(&self.resources[owner].source) {
                continue;
            }
            match self.resolve_owner_relationships(owner) {
                Ok(kind) => self.resources[owner].kind = kind,
                Err(error) if self.allow_partial && owner != 0 && !is_fatal_load_error(error.as_ref()) => self.disable_owner(owner, "Coworker or dependency disabled because its required configuration is invalid."),
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn resolve_owner_relationships(&mut self, owner: usize) -> CwrResult<ResourceKind> {
        let kind = self.resources[owner].kind.clone();
        Ok(match kind {
            ResourceKind::Coworker(mut coworker) => {
                for import in coworker
                    .imports
                    .iter_mut()
                    .filter(|import| import.default_conversation_target)
                {
                    match self.resolve_relationship(owner, &mut import.locator) {
                        Ok(()) => {}
                        Err(error)
                            if self.allow_partial && !is_fatal_load_error(error.as_ref()) =>
                        {
                            import.default_conversation_target = false;
                            self.warn(
                                self.resources[owner].source.clone(),
                                "The default coworker is unavailable. Choose another coworker.",
                            );
                        }
                        Err(error) => return Err(error),
                    }
                }
                for role in &mut coworker.roles {
                    self.resolve_relationship(owner, role)?;
                }
                for binding in &mut coworker.integrations {
                    match self.resolve_relationship(owner, &mut binding.locator) {
                        Ok(()) => {}
                        Err(error)
                            if self.allow_partial && !is_fatal_load_error(error.as_ref()) =>
                        {
                            binding.locator.reference = None;
                            binding.locator.path = None;
                            self.warn(self.resources[owner].source.clone(),
                                "A configured binding is unavailable; other bindings can still be used.");
                        }
                        Err(error) => return Err(error),
                    }
                }
                ResourceKind::Coworker(coworker)
            }
            ResourceKind::Role(mut role) => {
                for binding in &mut role.adapters {
                    match self.resolve_relationship(owner, &mut binding.locator) {
                        Ok(()) => {}
                        Err(error)
                            if self.allow_partial && !is_fatal_load_error(error.as_ref()) =>
                        {
                            binding.locator.reference = None;
                            binding.locator.path = None;
                            self.warn(self.resources[owner].source.clone(),
                                "A configured binding is unavailable; other bindings can still be used.");
                        }
                        Err(error) => return Err(error),
                    }
                }
                ResourceKind::Role(role)
            }
            ResourceKind::Integration(mut integration) => {
                self.resolve_relationship(owner, &mut integration.adapter)?;
                ResourceKind::Integration(integration)
            }
            ResourceKind::Gateway(mut gateway) => {
                self.resolve_relationship(owner, &mut gateway.role)?;
                self.resolve_relationship(owner, &mut gateway.integration)?;
                ResourceKind::Gateway(gateway)
            }
            other => other,
        })
    }

    fn resolve_relationship(
        &mut self,
        owner: usize,
        locator: &mut ResourceObject,
    ) -> CwrResult<()> {
        let target = self
            .load_locator(owner, locator, Visibility::Import)?
            .ok_or_else(|| invalid_error("relationship ref did not resolve"))?;
        if let ResolvedTarget::Resource(index) = &target {
            if self.disabled.contains(&self.resources[*index].source) {
                return invalid("required dependency is disabled");
            }
        }
        locator.path = None;
        locator.reference = Some(match target {
            ResolvedTarget::Resource(index) => self.resources[index].effective_ref.clone(),
            ResolvedTarget::External(reference) => reference,
        });
        Ok(())
    }

    fn reuse_target(&mut self, target: &ResolvedTarget, inputs: &InputBindings) -> CwrResult<bool> {
        let ResolvedTarget::Resource(index) = target else {
            return Ok(true);
        };
        if self.disabled.contains(&self.resources[*index].source) {
            return Ok(false);
        }
        match self.check_reused_inputs(*index, inputs) {
            Ok(()) => Ok(true),
            Err(error) if self.allow_partial && error.is::<ReceiverInputError>() => {
                self.warn(
                    self.resources[*index].source.clone(),
                    "Binding unavailable because its supplied inputs are invalid.",
                );
                Ok(false)
            }
            Err(error)
                if self.allow_partial && *index != 0 && !is_fatal_load_error(error.as_ref()) =>
            {
                self.disable_owner(
                    *index,
                    "Resource disabled because its configured inputs conflict.",
                );
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    pub(super) fn load_locator(
        &mut self,
        owner: usize,
        locator: &ResourceObject,
        visibility: Visibility,
    ) -> CwrResult<Option<ResolvedTarget>> {
        let inputs = forward_bindings(&locator.inputs, &self.bindings[owner])?;
        let source = self.resources[owner].source.clone();
        let target = match (&locator.path, &locator.reference) {
            (Some(path), None) => {
                if !(path.starts_with("./") || path.starts_with("../")) {
                    return invalid(format!(
                        "{}: dependency path must start with ./ or ../",
                        source.display()
                    ));
                }
                let path = source.parent().unwrap().join(path);
                let Some(index) = self.load_dependency(&path, inputs, visibility)? else {
                    return Ok(None);
                };
                ResolvedTarget::Resource(index)
            }
            (None, Some(reference)) => {
                validate_identity_ref(&source, reference)?;
                let reference = resolve_mount_ref(&self.scopes[owner].root_ref, reference);
                let known = self.scopes[owner]
                    .lookup
                    .get(&reference)
                    .cloned()
                    .or_else(|| {
                        self.resources
                            .iter()
                            .position(|resource| resource.effective_ref == reference)
                            .map(ResolvedTarget::Resource)
                    });
                if let Some(target) = known {
                    if let ResolvedTarget::Resource(index) = &target {
                        if self.disabled.contains(&self.resources[*index].source) {
                            return Ok(None);
                        }
                        // A gateway relationship without a mapping selects the already
                        // configured resource; its import owns the input bindings.
                        let selects_configured = matches!(&self.resources[owner].kind,
                            ResourceKind::Gateway(gateway) if locator.inputs.as_object().is_some_and(|inputs| inputs.is_empty())
                                && (locator.reference == gateway.role.reference
                                    || locator.reference == gateway.integration.reference));
                        if !selects_configured && !self.reuse_target(&target, &inputs)? {
                            return Ok(None);
                        }
                    }
                    target
                } else if is_builtin_adapter(&reference) {
                    if !inputs.is_empty() {
                        return invalid("bundled adapters do not accept configuration inputs");
                    }
                    ResolvedTarget::External(reference)
                } else {
                    let Some(path) = self.host.resolve(&reference)? else {
                        return Ok(None);
                    };
                    let Some(index) = self.load_dependency(&path, inputs, visibility)? else {
                        return Ok(None);
                    };
                    ResolvedTarget::Resource(index)
                }
            }
            _ => {
                return invalid(format!(
                    "{}: dependency must contain exactly one of path or ref",
                    source.display()
                ));
            }
        };
        if let Some(reference) = &locator.reference {
            let requested = resolve_mount_ref(&self.scopes[owner].root_ref, reference);
            insert_target(
                &source,
                &mut self.scopes[owner].lookup,
                requested,
                target.clone(),
            )?;
        }
        if let Some(alias) = &locator.alias {
            if let ResolvedTarget::Resource(child) = &target {
                for (name, exported) in self.exports(*child, &mut BTreeSet::new())? {
                    let reference = format!("{}/{alias}/{name}", self.scopes[owner].root_ref);
                    insert_target(&source, &mut self.scopes[owner].lookup, reference, exported)?;
                }
            }
            let alias_ref = contextual_alias(&self.scopes[owner].root_ref, alias);
            insert_target(
                &source,
                &mut self.scopes[owner].lookup,
                alias_ref,
                target.clone(),
            )?;
        }
        let canonical = match &target {
            ResolvedTarget::Resource(index) => self.resources[*index].effective_ref.clone(),
            ResolvedTarget::External(reference) => reference.clone(),
        };
        insert_target(
            &source,
            &mut self.scopes[owner].lookup,
            canonical,
            target.clone(),
        )?;
        if let ResolvedTarget::Resource(child) = target {
            self.edges
                .push((owner, child, locator.alias.clone(), visibility));
        }
        Ok(Some(target))
    }

    fn refresh_exports(&mut self) -> CwrResult<()> {
        // Exports expose names through an importing alias, without moving child namespaces.
        for owner in 0..self.scopes.len() {
            for (_, child, alias, _) in self
                .edges
                .iter()
                .filter(|(parent, _, _, _)| *parent == owner)
            {
                let Some(alias) = alias else {
                    continue;
                };
                let exports = self.exports(*child, &mut BTreeSet::new())?;
                for (name, target) in exports {
                    let reference = format!("{}/{alias}/{name}", self.scopes[owner].root_ref);
                    let source = self.scopes[owner].source.clone();
                    insert_target(&source, &mut self.scopes[owner].lookup, reference, target)?;
                }
            }
        }
        Ok(())
    }

    pub(super) fn finish(mut self, root_scope: usize) -> CwrResult<ResolvedGraph> {
        self.refresh_exports()?;
        let mut coworker_volumes = BTreeMap::new();
        for (source, volumes) in &self.resolved_volumes {
            let owner = self.by_source[source];
            let mut pending = vec![owner];
            let mut visited = BTreeSet::new();
            while let Some(index) = pending.pop() {
                if !visited.insert(index) {
                    continue;
                }
                if index != owner
                    && self
                        .resolved_volumes
                        .contains_key(&self.resources[index].source)
                {
                    continue;
                }
                if matches!(
                    self.resources[index].kind,
                    ResourceKind::Coworker(_) | ResourceKind::Gateway(_)
                ) {
                    let handler_source = self.resources[index].source.clone();
                    if coworker_volumes
                        .insert(handler_source.clone(), volumes.clone())
                        .is_some()
                    {
                        return invalid(format!(
                            "{}: execution handler is shared by multiple volume-owning runtimes",
                            handler_source.display()
                        ));
                    }
                }
                pending.extend(
                    self.edges
                        .iter()
                        .filter(|(owner, _, _, _)| *owner == index)
                        .filter_map(|(_, child, _, _)| {
                            matches!(
                                self.resources[*child].kind,
                                ResourceKind::Coworker(_)
                                    | ResourceKind::Gateway(_)
                                    | ResourceKind::Other
                            )
                            .then_some(*child)
                        }),
                );
            }
            if self.edges.iter().any(|(parent, child, _, _)| {
                *child != owner && visited.contains(child) && !visited.contains(parent)
            }) {
                return invalid(format!(
                    "{}: volume-owning runtime shares a resource outside its imports",
                    source.display()
                ));
            }
        }
        Ok(ResolvedGraph {
            root_scope,
            scopes: self.scopes,
            resources: self.resources,
            working_path: self.working_path,
            source_roots: self.roots,
            coworker_volumes,
            disabled: self.disabled,
            warnings: self.warnings,
        })
    }

    // Loading failures remove only the selected dependency. They never grant
    // authority or substitute incomplete configuration for an executable route.
    fn load_dependency(
        &mut self,
        path: &Path,
        inputs: InputBindings,
        visibility: Visibility,
    ) -> CwrResult<Option<usize>> {
        match self.load_file(path, inputs, visibility) {
            Ok(index) => Ok(Some(index)),
            Err(error) if self.allow_partial && !is_fatal_load_error(error.as_ref()) => {
                let source = path.canonicalize().unwrap_or_else(|_| path.to_owned());
                if self
                    .resources
                    .first()
                    .is_some_and(|root| root.source == source)
                {
                    return Err(error);
                }
                if error.is::<ReceiverInputError>() {
                    self.receiver_failed_sources.insert(source.clone());
                    self.warn(
                        source,
                        "Binding unavailable because its supplied inputs are invalid.",
                    );
                } else {
                    self.disabled.insert(source.clone());
                    self.warn(
                        source,
                        "Resource disabled because its declaration is missing or invalid.",
                    );
                }
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    fn disable_owner(&mut self, owner: usize, message: &str) {
        let source = self.resources[owner].source.clone();
        self.disabled.insert(source.clone());
        self.warn(source, message);
    }

    pub(super) fn warn(&mut self, source: PathBuf, message: &str) {
        let warning = GraphLoadWarning {
            source,
            message: message.into(),
        };
        if !self.warnings.contains(&warning) {
            self.warnings.push(warning);
        }
    }

    fn exports(
        &self,
        owner: usize,
        active: &mut BTreeSet<usize>,
    ) -> CwrResult<BTreeMap<String, ResolvedTarget>> {
        if !active.insert(owner) {
            return Ok(BTreeMap::new());
        }
        let mut names = BTreeMap::new();
        for (_, child, alias, _) in self.edges.iter().filter(|(parent, _, _, visibility)| {
            *parent == owner && matches!(visibility, Visibility::Export)
        }) {
            if let Some(alias) = alias {
                insert_target(
                    &self.scopes[owner].source,
                    &mut names,
                    alias.clone(),
                    ResolvedTarget::Resource(*child),
                )?;
            }
            for (name, target) in self.exports(*child, active)? {
                let name = alias
                    .as_ref()
                    .map_or(name.clone(), |alias| format!("{alias}/{name}"));
                insert_target(&self.scopes[owner].source, &mut names, name, target)?;
            }
        }
        active.remove(&owner);
        Ok(names)
    }
}

impl Loader<'_> {
    pub(super) fn has_unmatched_volume_grant(&self) -> bool {
        self.imported_volumes.keys().any(|key| {
            !self.used_imported_volumes.contains(key)
                && !self.disabled.contains(&key.0)
                && (!self.receiver_failed_sources.contains(&key.0)
                    || self.by_source.contains_key(&key.0))
        })
    }
}
