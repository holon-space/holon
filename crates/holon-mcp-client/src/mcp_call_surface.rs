//! Thin trait abstraction over the rmcp `Peer<RoleClient>` methods used by the
//! FDW. Exists so tests can drive the MCP fan-out logic with a scripted peer
//! instead of standing up a live rmcp transport.
//!
//! Only the two methods actually called by `McpCursor` are abstracted —
//! `call_tool` and `read_resource`. Everything else on `Peer` is unused by the
//! FDW and intentionally not part of this surface.

use async_trait::async_trait;
use rmcp::RoleClient;
use rmcp::model::{
    CallToolRequestParam, CallToolResult, ReadResourceRequestParam, ReadResourceResult,
};
use rmcp::service::{Peer, ServiceError};

#[async_trait]
pub trait McpCallSurface: Send + Sync + std::fmt::Debug {
    async fn call_tool(&self, params: CallToolRequestParam)
    -> Result<CallToolResult, ServiceError>;

    async fn read_resource(
        &self,
        params: ReadResourceRequestParam,
    ) -> Result<ReadResourceResult, ServiceError>;
}

#[async_trait]
impl McpCallSurface for Peer<RoleClient> {
    async fn call_tool(
        &self,
        params: CallToolRequestParam,
    ) -> Result<CallToolResult, ServiceError> {
        Peer::<RoleClient>::call_tool(self, params).await
    }

    async fn read_resource(
        &self,
        params: ReadResourceRequestParam,
    ) -> Result<ReadResourceResult, ServiceError> {
        Peer::<RoleClient>::read_resource(self, params).await
    }
}
