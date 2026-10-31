//! The only place that knows OpenTelemetry's `gen_ai.*` names, which are still in development.
//! If they change, change them here.

use crate::Kind;

pub const OPERATION_AGENT: &str = "invoke_agent";
pub const OPERATION_MODEL: &str = "chat";
pub const OPERATION_TOOL: &str = "execute_tool";

pub const ATTR_MODEL: &str = "gen_ai.request.model";
pub const ATTR_PROVIDER: &str = "gen_ai.provider.name";
pub const ATTR_INPUT_TOKENS: &str = "gen_ai.usage.input_tokens";
pub const ATTR_OUTPUT_TOKENS: &str = "gen_ai.usage.output_tokens";

/// Clio's own names (not in the OpenTelemetry convention).
pub const ATTR_PROMPT_VERSION: &str = "clio.prompt.version";
pub const NAME_CONTRACT_STATE: &str = "contract.state";
pub const ATTR_CONTRACT: &str = "contract";
pub const ATTR_FROM: &str = "from";
pub const ATTR_TO: &str = "to";

pub(crate) fn operation(kind: Kind) -> &'static str {
    match kind {
        Kind::Agent => OPERATION_AGENT,
        Kind::Model => OPERATION_MODEL,
        Kind::Tool => OPERATION_TOOL,
    }
}
