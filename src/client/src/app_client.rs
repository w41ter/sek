// Copyright 2022 The Engula Authors.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::{sync::Arc, time::Duration};

use engula_api::{
    server::v1::{group_request_union::Request, group_response_union::Response, *},
    v1::{create_collection_request::*, *},
};

use crate::{
    conn_manager::ConnManager, discovery::StaticServiceDiscovery, group_client::GroupClient,
    metrics::*, record_latency, AdminRequestBuilder, AdminResponseExtractor, AppError, AppResult,
    RetryState, RootClient, Router,
};

#[derive(Debug, Clone, Default)]
pub struct ClientOptions {
    /// The duration of connection timeout, an error is issued if establish connection is not
    /// finished after so the duration.
    pub connect_timeout: Option<Duration>,

    /// The duration of RPC over this client.
    pub timeout: Option<Duration>,
}

#[derive(Debug, Clone)]
pub struct Client {
    inner: Arc<ClientInner>,
}

#[derive(Debug, Clone)]
struct ClientInner {
    opts: ClientOptions,
    root_client: RootClient,
    router: Router,
    conn_manager: ConnManager,
}

impl Client {
    pub async fn new(opts: ClientOptions, addrs: Vec<String>) -> AppResult<Self> {
        let conn_manager = if let Some(connect_timeout) = opts.connect_timeout {
            ConnManager::with_connect_timeout(connect_timeout)
        } else {
            ConnManager::new()
        };

        let discovery = Arc::new(StaticServiceDiscovery::new(addrs.clone()));
        let root_client = RootClient::new(discovery, conn_manager.clone());
        let router = Router::new(root_client.clone()).await;
        Ok(Self {
            inner: Arc::new(ClientInner {
                opts,
                root_client,
                router,
                conn_manager,
            }),
        })
    }

    pub fn build(
        opts: ClientOptions,
        router: Router,
        root_client: RootClient,
        conn_manager: ConnManager,
    ) -> Self {
        Client {
            inner: Arc::new(ClientInner {
                opts,
                root_client,
                router,
                conn_manager,
            }),
        }
    }

    pub async fn create_database(&self, name: String) -> AppResult<Database> {
        let root_client = self.inner.root_client.clone();
        let resp = root_client
            .admin(AdminRequestBuilder::create_database(name.clone()))
            .await?;
        match AdminResponseExtractor::create_database(resp) {
            None => Err(AppError::NotFound(format!("database {name}"))),
            Some(desc) => Ok(Database {
                rpc_timeout: self.inner.opts.timeout,
                desc,
                client: self.clone(),
            }),
        }
    }

    pub async fn delete_database(&self, name: String) -> AppResult<()> {
        let root_client = self.inner.root_client.clone();
        let resp = root_client
            .admin(AdminRequestBuilder::delete_database(name.clone()))
            .await?;
        match AdminResponseExtractor::delete_database(resp) {
            Some(()) => Ok(()),
            None => Err(AppError::NotFound(format!("database {name}"))),
        }
    }

    pub async fn list_database(&self) -> AppResult<Vec<Database>> {
        let root_client = self.inner.root_client.clone();
        let resp = root_client
            .admin(AdminRequestBuilder::list_database())
            .await?;
        Ok(AdminResponseExtractor::list_database(resp)
            .into_iter()
            .map(|desc| Database {
                rpc_timeout: self.inner.opts.timeout,
                desc,
                client: self.clone(),
            })
            .collect::<Vec<_>>())
    }

    pub async fn open_database(&self, name: String) -> AppResult<Database> {
        let root_client = self.inner.root_client.clone();
        let resp = root_client
            .admin(AdminRequestBuilder::get_database(name.clone()))
            .await?;
        match AdminResponseExtractor::get_database(resp) {
            None => Err(AppError::NotFound(format!("database {}", name))),
            Some(desc) => Ok(Database {
                rpc_timeout: self.inner.opts.timeout,
                desc,
                client: self.clone(),
            }),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Database {
    client: Client,
    desc: DatabaseDesc,
    rpc_timeout: Option<Duration>,
}

pub enum Partition {
    Hash { slots: u32 },
    Range,
}

impl From<Partition> for create_collection_request::Partition {
    fn from(p: Partition) -> Self {
        match p {
            Partition::Hash { slots } => {
                create_collection_request::Partition::Hash(HashPartition { slots })
            }
            Partition::Range => create_collection_request::Partition::Range(RangePartition {}),
        }
    }
}

impl From<create_collection_request::Partition> for Partition {
    fn from(p: create_collection_request::Partition) -> Self {
        match p {
            create_collection_request::Partition::Hash(HashPartition { slots }) => {
                Partition::Hash { slots }
            }
            create_collection_request::Partition::Range(RangePartition {}) => Partition::Range,
        }
    }
}

impl Database {
    pub fn new(client: Client, desc: DatabaseDesc, rpc_timeout: Option<Duration>) -> Self {
        Database {
            client,
            desc,
            rpc_timeout,
        }
    }

    pub async fn create_collection(
        &self,
        name: String,
        partition: Option<Partition>,
    ) -> AppResult<Collection> {
        let client = self.client.clone();
        let db_desc = self.desc.clone();
        let root_client = client.inner.root_client.clone();
        let resp = root_client
            .admin(AdminRequestBuilder::create_collection(
                db_desc,
                name.clone(),
                partition.map(Into::into),
            ))
            .await?;
        match AdminResponseExtractor::create_collection(resp) {
            None => Err(AppError::NotFound(format!("collection {name}"))),
            Some(co_desc) => Ok(Collection {
                rpc_timeout: self.rpc_timeout,
                co_desc,
                client: client.clone(),
            }),
        }
    }

    pub async fn delete_collection(&self, name: String) -> AppResult<()> {
        let client = self.client.clone();
        let db_desc = self.desc.clone();
        let root_client = client.inner.root_client.clone();
        let resp = root_client
            .admin(AdminRequestBuilder::delete_collection(
                db_desc.clone(),
                name.clone(),
            ))
            .await?;
        match AdminResponseExtractor::delete_collection(resp) {
            None => Err(AppError::NotFound(format!("collection {name}"))),
            Some(_) => Ok(()),
        }
    }

    pub async fn list_collection(&self) -> AppResult<Vec<Collection>> {
        let client = self.client.clone();
        let root_client = client.inner.root_client.clone();
        let resp = root_client
            .admin(AdminRequestBuilder::list_collection(self.desc.clone()))
            .await?;
        Ok(AdminResponseExtractor::list_collection(resp)
            .into_iter()
            .map(|co_desc| Collection {
                rpc_timeout: self.rpc_timeout,
                co_desc,
                client: client.clone(),
            })
            .collect::<Vec<_>>())
    }

    pub async fn open_collection(&self, name: String) -> AppResult<Collection> {
        let client = self.client.clone();
        let db_desc = self.desc.clone();
        let root_client = client.inner.root_client.clone();
        let resp = root_client
            .admin(AdminRequestBuilder::get_collection(db_desc, name.clone()))
            .await?;
        match AdminResponseExtractor::get_collection(resp) {
            None => Err(AppError::NotFound(format!("collection {}", name))),
            Some(co_desc) => Ok(Collection {
                rpc_timeout: self.rpc_timeout,
                co_desc,
                client: client.clone(),
            }),
        }
    }

    #[allow(dead_code)]
    pub fn name(&self) -> String {
        self.desc.name.to_owned()
    }

    #[inline]
    pub fn desc(&self) -> DatabaseDesc {
        self.desc.clone()
    }
}

#[derive(Debug, Clone)]
pub struct Collection {
    client: Client,
    co_desc: CollectionDesc,
    rpc_timeout: Option<Duration>,
}

impl Collection {
    pub fn new(
        client: Client,
        co_desc: CollectionDesc,
        rpc_timeout: Option<Duration>,
    ) -> Collection {
        Collection {
            client,
            co_desc,
            rpc_timeout,
        }
    }

    pub async fn delete(&self, key: Vec<u8>) -> AppResult<()> {
        CLIENT_DATABASE_BYTES_TOTAL.rx.inc_by(key.len() as u64);
        CLIENT_DATABASE_REQUEST_TOTAL.delete.inc();
        record_latency!(&CLIENT_DATABASE_REQUEST_DURATION_SECONDS.get);
        let mut retry_state = RetryState::new(self.rpc_timeout);

        loop {
            match self.delete_inner(&key, retry_state.timeout()).await {
                Ok(()) => return Ok(()),
                Err(err) => {
                    retry_state.retry(err).await?;
                }
            }
        }
    }

    pub async fn put(&self, key: Vec<u8>, value: Vec<u8>) -> AppResult<()> {
        CLIENT_DATABASE_BYTES_TOTAL
            .rx
            .inc_by((key.len() + value.len()) as u64);
        CLIENT_DATABASE_REQUEST_TOTAL.put.inc();
        record_latency!(&CLIENT_DATABASE_REQUEST_DURATION_SECONDS.put);
        let mut retry_state = RetryState::new(self.rpc_timeout);

        loop {
            match self.put_inner(&key, &value, retry_state.timeout()).await {
                Ok(()) => return Ok(()),
                Err(err) => {
                    retry_state.retry(err).await?;
                }
            }
        }
    }

    pub async fn get(&self, key: Vec<u8>) -> AppResult<Option<Vec<u8>>> {
        CLIENT_DATABASE_BYTES_TOTAL.rx.inc_by(key.len() as u64);
        CLIENT_DATABASE_REQUEST_TOTAL.get.inc();
        record_latency!(&CLIENT_DATABASE_REQUEST_DURATION_SECONDS.get);
        let mut retry_state = RetryState::new(self.rpc_timeout);

        loop {
            match self.get_inner(&key, retry_state.timeout()).await {
                Ok(value) => {
                    CLIENT_DATABASE_BYTES_TOTAL
                        .tx
                        .inc_by(value.as_ref().map(Vec::len).unwrap_or_default() as u64);
                    return Ok(value);
                }
                Err(err) => {
                    retry_state.retry(err).await?;
                }
            }
        }
    }

    async fn delete_inner(&self, key: &[u8], timeout: Option<Duration>) -> crate::Result<()> {
        let router = self.client.inner.router.clone();
        let (group, shard) = router.find_shard(self.co_desc.clone(), key)?;
        let mut client = GroupClient::new(
            group,
            self.client.inner.router.clone(),
            self.client.inner.conn_manager.clone(),
        );
        let req = Request::Delete(ShardDeleteRequest {
            shard_id: shard.id,
            delete: Some(DeleteRequest {
                key: key.to_owned(),
            }),
        });
        if let Some(duration) = timeout {
            client.set_timeout(duration);
        }
        client.request(&req).await?;
        Ok(())
    }

    async fn put_inner(
        &self,
        key: &[u8],
        value: &[u8],
        timeout: Option<Duration>,
    ) -> crate::Result<()> {
        let router = self.client.inner.router.clone();
        let (group, shard) = router.find_shard(self.co_desc.clone(), key)?;
        let mut client = GroupClient::new(
            group,
            self.client.inner.router.clone(),
            self.client.inner.conn_manager.clone(),
        );
        let req = Request::Put(ShardPutRequest {
            shard_id: shard.id,
            put: Some(PutRequest {
                key: key.to_owned(),
                value: value.to_owned(),
            }),
        });
        if let Some(duration) = timeout {
            client.set_timeout(duration);
        }
        client.request(&req).await?;
        Ok(())
    }

    async fn get_inner(
        &self,
        key: &[u8],
        timeout: Option<Duration>,
    ) -> crate::Result<Option<Vec<u8>>> {
        let router = self.client.inner.router.clone();
        let (group, shard) = router.find_shard(self.co_desc.clone(), key)?;
        let mut client = GroupClient::new(
            group,
            self.client.inner.router.clone(),
            self.client.inner.conn_manager.clone(),
        );
        let req = Request::Get(ShardGetRequest {
            shard_id: shard.id,
            get: Some(GetRequest {
                key: key.to_owned(),
            }),
        });
        if let Some(duration) = timeout {
            client.set_timeout(duration);
        }
        match client.request(&req).await? {
            Response::Get(GetResponse { value }) => Ok(value),
            _ => Err(crate::Error::Internal(wrap(
                "invalid response type, Get is required",
            ))),
        }
    }

    #[allow(dead_code)]
    fn name(&self) -> String {
        self.co_desc.name.to_owned()
    }

    #[inline]
    pub fn desc(&self) -> CollectionDesc {
        self.co_desc.clone()
    }
}

#[inline]
fn wrap(msg: &str) -> Box<dyn std::error::Error + Sync + Send + 'static> {
    let msg = String::from(msg);
    msg.into()
}
