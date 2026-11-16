struct ExecLiveOutput<'a> {
    handle: Option<&'a str>,
    revision: u64,
    identity: Option<(String, String, String)>,
    text: String,
    line_open: bool,
    streamed_final_answer: bool,
}

impl<'a> ExecLiveOutput<'a> {
    fn new(handle: Option<&'a str>) -> Self {
        Self {
            handle,
            revision: 0,
            identity: None,
            text: String::new(),
            line_open: false,
            streamed_final_answer: false,
        }
    }

    fn refresh(&mut self, mailbox: &AdapterModelOutputMailbox) -> io::Result<()> {
        let Some(handle) = self.handle else {
            return Ok(());
        };
        let snapshot = mailbox.snapshot();
        if snapshot.revision == self.revision {
            return Ok(());
        }
        self.revision = snapshot.revision;
        let Some(output) = snapshot.output else {
            return self.finish_line();
        };
        let identity = output_identity(&output);
        let suffix = (self.identity.as_ref() == Some(&identity) && self.line_open)
            .then(|| output.text.strip_prefix(&self.text))
            .flatten();
        let mut stderr = io::stderr().lock();
        if let Some(suffix) = suffix {
            stderr.write_all(suffix.as_bytes())?;
        } else {
            if self.line_open {
                stderr.write_all(b"\n")?;
            }
            write!(stderr, "{handle}: {}", output.text)?;
        }
        stderr.flush()?;
        self.identity = Some(identity);
        self.text = output.text;
        self.line_open = true;
        self.streamed_final_answer |=
            output.phase == cwr::open_responses::v2026_04_24::AssistantPhase::FinalAnswer;
        Ok(())
    }

    fn matches_completed_commentary(&self, event: &LocalRuntimeEvent) -> bool {
        let LocalRuntimeEvent::Activity { activity, .. } = event else {
            return false;
        };
        let ConversationActivityEvent::CommentaryProgress {
            model_run_ref,
            response_or_turn_id,
            item_id,
            text,
            ..
        } = &activity.event
        else {
            return false;
        };
        self.identity.as_ref()
            == Some(&(
                model_run_ref.clone(),
                response_or_turn_id.clone(),
                item_id.clone(),
            ))
            && self.text == *text
    }

    fn finish_line(&mut self) -> io::Result<()> {
        if self.line_open {
            let mut stderr = io::stderr().lock();
            stderr.write_all(b"\n")?;
            stderr.flush()?;
            self.line_open = false;
        }
        Ok(())
    }
}

fn output_identity(output: &AdapterLiveOutput) -> (String, String, String) {
    (
        output.model_run_ref.clone(),
        output.response_or_turn_id.clone(),
        output.item_id.clone(),
    )
}
