// Copyright © Aptos Foundation
// SPDX-License-Identifier: Apache-2.0

use crate::{
    asset_uploader::{config::AssetUploaderConfig, AssetUploaderContext},
    parser::{config::ParserConfig, ParserContext},
    utils::database::{establish_connection_pool, run_migrations},
};
use aptos_indexer_grpc_server_framework::RunnableConfig;
use axum::Router;
use diesel::{
    r2d2::{ConnectionManager, Pool},
    PgConnection,
};
use enum_dispatch::enum_dispatch;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tracing::info;

/// Trait for building a router for axum
#[enum_dispatch]
pub trait Server: Send + Sync {
    fn build_router(&self) -> Router;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerConfig {
    Parser(ParserConfig),
    AssetUploader(AssetUploaderConfig),
}

/// Structs to hold config from YAML
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NFTMetadataCrawlerConfig {
    pub database_url: String,
    pub server_port: u16,
    pub server_config: ServerConfig,
}

#[derive(Clone)]
#[enum_dispatch(Server)]
pub enum ServerContext {
    Parser(ParserContext),
    AssetUploader(AssetUploaderContext),
}

impl ServerConfig {
    pub async fn build_context(
        &self,
        pool: Pool<ConnectionManager<PgConnection>>,
    ) -> ServerContext {
        match self {
            ServerConfig::Parser(parser_config) => {
                ServerContext::Parser(ParserContext::new(parser_config.clone(), pool).await)
            },
            ServerConfig::AssetUploader(asset_uploader_config) => ServerContext::AssetUploader(
                AssetUploaderContext::new(asset_uploader_config.clone(), pool),
            ),
        }
    }
}

#[async_trait::async_trait]
impl RunnableConfig for NFTMetadataCrawlerConfig {
    /// Main driver function that establishes a connection to Pubsub and parses the Pubsub entries in parallel
    async fn run(&self) -> anyhow::Result<()> {
        info!("[NFT Metadata Crawler] Starting with config: {:?}", self);

        info!("[NFT Metadata Crawler] Connecting to database");
        let pool = establish_connection_pool(&self.database_url);
        info!("[NFT Metadata Crawler] Database connection successful");

        info!("[NFT Metadata Crawler] Running migrations");
        run_migrations(&pool);
        info!("[NFT Metadata Crawler] Finished migrations");

        // Create request context
        let context = self.server_config.build_context(pool).await;
        let listener = TcpListener::bind(format!("0.0.0.0:{}", self.server_port)).await?;
        axum::serve(listener, context.build_router()).await?;

        Ok(())
    }

    fn get_server_name(&self) -> String {
        match &self.server_config {
            ServerConfig::Parser(_) => "parser".to_string(),
            ServerConfig::AssetUploader(_) => "asset_uploader".to_string(),
        }
    }
}
