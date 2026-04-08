use std::borrow::Cow;

use rmcp::ErrorData;
use rmcp::handler::server::router::tool::{AsyncTool, ToolBase};
use rmcp::schemars;
use serde::{Deserialize, Serialize};

use crate::server::{NephilaMcpServer, nephila_err, parse_objective_id};
use nephila_core::event::BusEvent;
use nephila_core::objective::ObjectiveStatus;
use nephila_core::store::ObjectiveStore;

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct GetObjectiveTreeParams {
    /// The root objective ID to retrieve the tree for.
    pub root_id: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct GetObjectiveTreeOutput {
    /// The objective tree serialized as JSON.
    pub tree_json: String,
}

pub struct GetObjectiveTreeTool;

impl ToolBase for GetObjectiveTreeTool {
    type Parameter = GetObjectiveTreeParams;
    type Output = GetObjectiveTreeOutput;
    type Error = ErrorData;

    fn name() -> Cow<'static, str> {
        "get_objective_tree".into()
    }

    fn description() -> Option<Cow<'static, str>> {
        Some("Get the objective tree starting from the given root.".into())
    }
}

impl AsyncTool<NephilaMcpServer> for GetObjectiveTreeTool {
    async fn invoke(
        service: &NephilaMcpServer,
        params: Self::Parameter,
    ) -> Result<Self::Output, Self::Error> {
        let root_id = parse_objective_id(&params.root_id)?;

        let tree = service
            .sqlite
            .get_tree(root_id)
            .await
            .map_err(nephila_err)?;
        let tree_json = serde_json::to_string(&tree)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        Ok(GetObjectiveTreeOutput { tree_json })
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct UpdateObjectiveParams {
    /// The objective ID to update.
    pub objective_id: String,
    /// New status: "pending", "in_progress", "done", or "blocked".
    pub status: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct UpdateObjectiveOutput {
    pub updated: bool,
}

pub struct UpdateObjectiveTool;

impl ToolBase for UpdateObjectiveTool {
    type Parameter = UpdateObjectiveParams;
    type Output = UpdateObjectiveOutput;
    type Error = ErrorData;

    fn name() -> Cow<'static, str> {
        "update_objective".into()
    }

    fn description() -> Option<Cow<'static, str>> {
        Some("Update an objective's status.".into())
    }
}

impl AsyncTool<NephilaMcpServer> for UpdateObjectiveTool {
    async fn invoke(
        service: &NephilaMcpServer,
        params: Self::Parameter,
    ) -> Result<Self::Output, Self::Error> {
        let objective_id = parse_objective_id(&params.objective_id)?;

        let status: ObjectiveStatus = params.status.parse().map_err(|_| {
            ErrorData::invalid_params(
                format!(
                    "invalid status: '{}' (expected: pending, in_progress, done, blocked)",
                    params.status
                ),
                None,
            )
        })?;

        service
            .sqlite
            .update_status(objective_id, status)
            .await
            .map_err(nephila_err)?;

        let _ = service.event_tx.send(BusEvent::ObjectiveUpdated {
            objective_id,
            status,
        });

        Ok(UpdateObjectiveOutput { updated: true })
    }
}
