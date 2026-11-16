use super::*;
impl Catalog {
    pub(super) fn mutate(&self, tx: &Transaction<'_>, request: &Request) -> Result<Response> {
        let request_id = request
            .request_id()
            .ok_or_else(|| error(Code::InvalidInput))?;
        let arguments = json!({"domain":"workspace_versions","request":request});
        match self.receipts.replay(tx, request_id, &arguments)? {
            Replay::Retained(response) => return Ok(response),
            Replay::Expired => return Ok(Response::CommittedResultExpired),
            Replay::Absent => {}
        }
        let root = request.root();
        let owner = instance_ref(root);
        match request {
            Request::Save {
                instance,
                expected_revision,
                ..
            } => {
                let before = self.read(tx, INSTANCE, &owner)?;
                if before
                    .as_ref()
                    .map(|record| record.revision.legacy_counter().to_string())
                    != *expected_revision
                {
                    return Err(error(Code::RevisionConflict));
                }
                if instance.name.is_empty()
                    || instance.name.chars().count() > 128
                    || instance.id.is_empty()
                {
                    return Err(error(Code::InvalidInput));
                }
                if let Some(version) = self.active(tx, root)? {
                    if instance.folders.as_ref() != Some(&version.folders) {
                        return Err(error(Code::InvalidValue));
                    }
                }
                if before.is_some() {
                    self.receipts.expire(tx, &owner)?;
                }
                self.save_instance_value(tx, root, instance)?;
                if let Some(version) = self.active(tx, root)? {
                    self.membership(tx, root, &version)?;
                }
            }
            Request::Rename {
                name,
                expected_revision,
                ..
            } => {
                let before = self
                    .read(tx, INSTANCE, &owner)?
                    .ok_or_else(|| error(Code::NotFound))?;
                if before.revision.legacy_counter().to_string() != *expected_revision {
                    return Err(error(Code::RevisionConflict));
                }
                if name.trim() != name
                    || name.is_empty()
                    || name.chars().count() > 128
                    || name.chars().any(|c| c.is_control())
                {
                    return Err(error(Code::InvalidInput));
                }
                let mut instance = self
                    .instance(tx, root)?
                    .ok_or_else(|| error(Code::NotFound))?;
                self.receipts.expire(tx, &owner)?;
                instance.name = name.clone();
                self.save_instance_value(tx, root, &instance)?;
            }
            Request::Publish {
                version, import, ..
            } => {
                self.instance(tx, root)?
                    .ok_or_else(|| error(Code::NotFound))?;
                self.receipts.expire(tx, &owner)?;
                self.publish(tx, root, version, *import)?;
            }
            Request::Import {
                versions,
                active,
                expected_active,
                ..
            } => {
                if versions.len() > 256 {
                    return Err(error(Code::ServiceLimit));
                }
                self.instance(tx, root)?
                    .ok_or_else(|| error(Code::NotFound))?;
                self.receipts.expire(tx, &owner)?;
                for version in versions {
                    self.publish(tx, root, version, true)?;
                }
                self.selection(
                    tx,
                    root,
                    active,
                    expected_active.as_deref(),
                    SelectionPolicy::Compare,
                )?;
            }
            Request::Select {
                version,
                expected_active,
                policy,
                ..
            } => {
                self.instance(tx, root)?
                    .ok_or_else(|| error(Code::NotFound))?;
                self.receipts.expire(tx, &owner)?;
                self.selection(tx, root, version, expected_active.as_deref(), *policy)?;
            }
            Request::Remove {
                expected_revision, ..
            } => {
                let before = self
                    .read(tx, INSTANCE, &owner)?
                    .ok_or_else(|| error(Code::NotFound))?;
                if before.revision.legacy_counter().to_string() != *expected_revision {
                    return Err(error(Code::RevisionConflict));
                }
                self.remove(tx, root)?;
            }
            _ => return Err(error(Code::InvalidInput)),
        }
        let response = Response::Committed;
        self.receipts.save(
            tx,
            request_id,
            &arguments,
            &response,
            if matches!(request, Request::Remove { .. }) {
                None
            } else {
                Some(owner.as_str())
            },
        )?;
        Ok(response)
    }
    pub(super) fn publish(
        &self,
        tx: &Transaction<'_>,
        root: &str,
        version: &Version,
        import: bool,
    ) -> Result<()> {
        if version.id.is_empty()
            || version.id.len() > 1024
            || version.files.len() > 4096
            || version.folders.len() > MAX_WORK
        {
            return Err(error(Code::ServiceLimit));
        }
        for file in &version.files {
            if file.path.contains('\0')
                || file.path.is_empty()
                || file.path.contains('\\')
                || std::path::Path::new(&file.path).is_absolute()
                || file.path.split('/').any(|part| part == ".." || part == ".")
                || file.mode > 511
            {
                return Err(error(Code::InvalidInput));
            }
        }
        let reference = version_ref(root, &version.id);
        if let Some(existing) = self.version(tx, root, &version.id)? {
            if !import {
                return Err(error(Code::AlreadyExists));
            }
            if existing != *version {
                return Err(error(Code::EdgeConflict));
            }
        } else {
            if unordered::retired(tx, &reference).map_err(storage)? {
                return Err(error(Code::RefRetired));
            }
            self.version
                .create_in_transaction(tx, VERSION, &reference, &json!({"record":version}))
                .map_err(storage)?;
        }
        self.membership(tx, root, version)?;
        Ok(())
    }
    fn remove(&self, tx: &Transaction<'_>, root: &str) -> Result<()> {
        let owner = instance_ref(root);
        let versions =
            unordered::many_outgoing(tx, VERSION_EDGE, &owner, MAX_WORK).map_err(relation)?;
        let grants =
            unordered::many_outgoing(tx, GRANT_EDGE, &owner, MAX_WORK).map_err(relation)?;
        let mut work = versions.len() + grants.len();
        let mut memberships = Vec::new();
        for version in &versions {
            let members = unordered::many_outgoing(tx, MEMBER_EDGE, &version.target, MAX_WORK)
                .map_err(relation)?;
            work += members.len();
            if work > MAX_WORK {
                return Err(error(Code::ServiceLimit));
            }
            memberships.push(members);
        }
        self.receipts.expire(tx, &owner)?;
        if let Some(active) =
            unordered::single_outgoing(tx, ACTIVE_EDGE, &owner).map_err(relation)?
        {
            unordered::unlink(tx, ACTIVE_EDGE, &owner, &active.target, active.revision)
                .map_err(relation)?;
        }
        for (version, members) in versions.iter().zip(memberships) {
            let record = self
                .read(tx, VERSION, &version.target)?
                .ok_or_else(|| error(Code::UnsupportedDomainContract))?;
            let value: Version = serde_json::from_value(record.value.properties["record"].clone())
                .map_err(|_| error(Code::UnsupportedDomainContract))?;
            if version.target != version_ref(root, &value.id) {
                return Err(error(Code::UnsupportedDomainContract));
            }
            for member in members {
                unordered::unlink(
                    tx,
                    MEMBER_EDGE,
                    &version.target,
                    &member.target,
                    member.revision,
                )
                .map_err(relation)?;
            }
            let ready = ready_ref(root, &value.id);
            if self.read(tx, READY, &ready)?.is_some() {
                unordered::delete_unlinked_entity(tx, &ready).map_err(relation)?;
            }
            unordered::unlink(tx, VERSION_EDGE, &owner, &version.target, version.revision)
                .map_err(relation)?;
            unordered::adopt_entity(tx, &version.target, VERSION_KIND).map_err(storage)?;
            unordered::delete_unlinked_entity(tx, &version.target).map_err(relation)?;
        }
        for grant in grants {
            self.read(tx, GRANT, &grant.target)?
                .ok_or_else(|| error(Code::UnsupportedDomainContract))?;
            unordered::unlink(tx, GRANT_EDGE, &owner, &grant.target, grant.revision)
                .map_err(relation)?;
            unordered::delete_unlinked_entity(tx, &grant.target).map_err(relation)?;
        }
        unordered::require_unlinked(tx, &owner).map_err(relation)?;
        let before = self
            .read(tx, INSTANCE, &owner)?
            .ok_or_else(|| error(Code::NotFound))?;
        unordered::adopt_entity(tx, &owner, INSTANCE_KIND).map_err(storage)?;
        self.instance
            .replace_in_transaction(
                tx,
                INSTANCE,
                &owner,
                &before.revision,
                &json!({"record":{"removed":true}}),
            )
            .map_err(storage)?;
        Ok(())
    }
}
