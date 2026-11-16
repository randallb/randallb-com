#[cfg(test)]
mod tests {
    use super::*;

    fn mapper() -> CodexOpenResponsesMapper {
        CodexOpenResponsesMapper::new("thread-1", "turn-1")
    }

    fn started(item_id: &str, phase: Option<&str>) -> Value {
        json!({
            "method": "item/started",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "startedAtMs": 1,
                "item": {
                    "type": "agentMessage",
                    "id": item_id,
                    "text": "",
                    "phase": phase,
                    "memoryCitation": null
                }
            }
        })
    }

    fn delta(item_id: &str, text: &str) -> Value {
        json!({
            "method": "item/agentMessage/delta",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": item_id,
                "delta": text
            }
        })
    }

    fn completed(item_id: &str, phase: Option<&str>, text: &str) -> Value {
        json!({
            "method": "item/completed",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "completedAtMs": 2,
                "item": {
                    "type": "agentMessage",
                    "id": item_id,
                    "text": text,
                    "phase": phase,
                    "memoryCitation": null
                }
            }
        })
    }

    #[test]
    fn maps_started_delta_and_completed_commentary_to_the_standard_lifecycle() {
        let mut mapper = mapper();

        let start = mapper
            .map_message(&started("message-1", Some("commentary")))
            .expect("map start");
        let update = mapper
            .map_message(&delta("message-1", "Working"))
            .expect("map delta");
        let done = mapper
            .map_message(&completed("message-1", Some("commentary"), "Working"))
            .expect("map completion");

        assert_eq!(start.events.len(), 2);
        assert_eq!(start.events[0].event["type"], "response.output_item.added");
        assert_eq!(start.events[0].event["item"]["phase"], "commentary");
        assert_eq!(start.events[1].event["type"], "response.content_part.added");
        assert!(start
            .events
            .iter()
            .all(|event| event.phase == AssistantPhase::Commentary));
        assert_eq!(update.events.len(), 1);
        assert_eq!(update.events[0].event["type"], "response.output_text.delta");
        assert_eq!(update.events[0].phase, AssistantPhase::Commentary);
        assert_eq!(done.events.len(), 3);
        assert_eq!(done.events[0].event["type"], "response.output_text.done");
        assert_eq!(done.events[1].event["type"], "response.content_part.done");
        assert_eq!(done.events[2].event["type"], "response.output_item.done");
        assert_eq!(done.events[2].event["item"]["phase"], "commentary");
        assert!(done
            .events
            .iter()
            .all(|event| event.phase == AssistantPhase::Commentary));
        let completed = done.completed_assistant.expect("completed assistant");
        assert_eq!(completed.phase, AssistantPhase::Commentary);
        assert_eq!(completed.text, "Working");
    }

    #[test]
    fn interleaved_items_keep_their_phase_and_output_identity() {
        let mut mapper = mapper();
        let commentary = mapper
            .map_message(&started("commentary-1", Some("commentary")))
            .expect("commentary start");
        let final_answer = mapper
            .map_message(&started("answer-1", Some("final_answer")))
            .expect("answer start");
        let commentary_delta = mapper
            .map_message(&delta("commentary-1", "Checking"))
            .expect("commentary delta");
        let answer_delta = mapper
            .map_message(&delta("answer-1", "Done"))
            .expect("answer delta");

        assert_eq!(commentary.events[0].event["output_index"], 0);
        assert_eq!(final_answer.events[0].event["output_index"], 1);
        assert_eq!(commentary_delta.events[0].event["output_index"], 0);
        assert_eq!(answer_delta.events[0].event["output_index"], 1);
        assert_eq!(commentary.events[0].phase, AssistantPhase::Commentary);
        assert_eq!(commentary_delta.events[0].phase, AssistantPhase::Commentary);
        assert_eq!(final_answer.events[0].phase, AssistantPhase::FinalAnswer);
        assert_eq!(answer_delta.events[0].phase, AssistantPhase::FinalAnswer);

        let commentary_done = mapper
            .map_message(&completed("commentary-1", Some("commentary"), "Checking"))
            .expect("commentary done")
            .completed_assistant
            .expect("commentary completion");
        let answer_done = mapper
            .map_message(&completed("answer-1", Some("final_answer"), "Done"))
            .expect("answer done")
            .completed_assistant
            .expect("answer completion");
        assert_eq!(commentary_done.phase, AssistantPhase::Commentary);
        assert_eq!(answer_done.phase, AssistantPhase::FinalAnswer);
    }

    #[test]
    fn completion_only_messages_get_a_complete_standard_snapshot_lifecycle() {
        let mut mapper = mapper();

        let mapped = mapper
            .map_message(&completed(
                "message-1",
                Some("commentary"),
                "Finished update",
            ))
            .expect("map completion-only item");

        assert_eq!(mapped.events.len(), 5);
        assert_eq!(mapped.events[0].event["type"], "response.output_item.added");
        assert_eq!(
            mapped.events[1].event["type"],
            "response.content_part.added"
        );
        assert_eq!(mapped.events[2].event["type"], "response.output_text.done");
        assert_eq!(mapped.events[3].event["type"], "response.content_part.done");
        assert_eq!(mapped.events[4].event["type"], "response.output_item.done");
        assert_eq!(
            mapped.completed_assistant.expect("completed item").text,
            "Finished update"
        );
    }

    #[test]
    fn completed_text_repairs_the_accumulated_deltas() {
        let mut mapper = mapper();
        mapper
            .map_message(&started("message-1", Some("commentary")))
            .expect("map start");
        mapper
            .map_message(&delta("message-1", "Incomplete dra"))
            .expect("map partial delta");

        let mapped = mapper
            .map_message(&completed(
                "message-1",
                Some("commentary"),
                "Corrected draft",
            ))
            .expect("map corrected completion");

        assert_eq!(mapped.events[0].event["text"], "Corrected draft");
        assert_eq!(mapped.events[1].event["part"]["text"], "Corrected draft");
        assert_eq!(
            mapped.events[2].event["item"]["content"][0]["text"],
            "Corrected draft"
        );
    }

    #[test]
    fn completion_reuses_retained_phase_and_rejects_conflicts_or_duplicates() {
        let mut mapper = mapper();
        mapper
            .map_message(&started("message-1", Some("commentary")))
            .expect("map start");

        assert!(mapper
            .map_message(&completed("message-1", Some("final_answer"), "Wrong phase",))
            .is_err());

        let completed_message = mapper
            .map_message(&completed("message-1", None, "Retained phase"))
            .expect("completion may omit an already-retained phase")
            .completed_assistant
            .expect("completed assistant");
        assert_eq!(completed_message.phase, AssistantPhase::Commentary);
        assert_eq!(completed_message.text, "Retained phase");

        assert!(mapper
            .map_message(&completed("message-1", Some("commentary"), "Duplicate",))
            .is_err());
    }

    #[test]
    fn missing_or_unknown_phase_and_unsupported_items_are_not_mapped() {
        let mut mapper = mapper();
        let mut missing_turn = started("missing-turn", Some("commentary"));
        missing_turn["params"]
            .as_object_mut()
            .expect("params")
            .remove("turnId");
        let mut missing_item = completed("missing-item", Some("commentary"), "Hidden");
        missing_item["params"]["item"]
            .as_object_mut()
            .expect("item")
            .remove("id");
        let mut wrong_thread = completed("wrong-thread", Some("commentary"), "Hidden");
        wrong_thread["params"]["threadId"] = json!("thread-2");
        let mut wrong_turn = completed("wrong-turn", Some("commentary"), "Hidden");
        wrong_turn["params"]["turnId"] = json!("turn-2");
        for message in [
            started("missing", None),
            started("future", Some("future_phase")),
            completed("missing", None, "Hidden"),
            completed("future", Some("future_phase"), "Hidden"),
            missing_turn,
            missing_item,
            wrong_thread,
            wrong_turn,
            json!({
                "method": "item/completed",
                "params": {
                    "threadId": "thread-1",
                    "turnId": "turn-1",
                    "item": {"type": "reasoning", "id": "reasoning-1"}
                }
            }),
        ] {
            let mapped = mapper
                .map_message(&message)
                .expect("ignore unsupported item");
            assert!(mapped.events.is_empty());
            assert!(mapped.completed_assistant.is_none());
        }
    }
}
