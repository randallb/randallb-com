use super::*;

pub(super) fn changed(
    previous: &AdapterInvocation,
    invocation: &AdapterInvocation,
    current_gateway: Option<&crate::adapter::AdapterHostOperation>,
) -> bool {
    let previous_gateway = previous
        .host_operations
        .iter()
        .find(|op| op.name == "cwr.send_request");
    let current_gateway = current_gateway.or_else(|| {
        invocation
            .host_operations
            .iter()
            .find(|op| op.name == "cwr.send_request")
    });
    contract(previous_gateway) != contract(current_gateway)
}

fn contract(operation: Option<&crate::adapter::AdapterHostOperation>) -> Option<Value> {
    operation.map(|operation| {
        let mut schema = operation.input_schema.clone();
        if let Some(object) = schema.as_object_mut() {
            // Model naming/decoding annotations describe transport, not access.
            object.remove("x-cwr-model-name");
            object.remove("x-cwr-decoder-version");
        }
        json!({"input":schema,"output":operation.output_schema})
    })
}
