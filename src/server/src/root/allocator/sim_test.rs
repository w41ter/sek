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

use std::{
    collections::{hash_map::Entry, HashMap},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};

use engula_api::server::v1::{GroupDesc, NodeCapacity, ReplicaRole, ShardDesc};

use super::*;
use crate::{bootstrap::REPLICA_PER_GROUP, runtime::ExecutorOwner};

#[test]
fn sim_boostrap_join_node_balance() {
    let executor_owner = ExecutorOwner::new(1);
    let executor = executor_owner.executor();
    executor.block_on(async {
        let p = Arc::new(MockInfoProvider::new());
        let a = Allocator::new(p.clone(), REPLICA_PER_GROUP);

        println!("1. boostrap and no need rebalance");
        p.set_groups(vec![GroupDesc {
            id: 1,
            epoch: 0,
            shards: vec![],
            replicas: vec![ReplicaDesc {
                id: 1,
                node_id: 1,
                role: ReplicaRole::Voter.into(),
            }],
        }]);
        p.set_nodes(vec![NodeDesc {
            id: 1,
            addr: "".into(),
            capacity: Some(NodeCapacity {
                cpu_nums: 4.0,
                replica_count: 1,
                leader_count: 1,
            }),
        }]);

        let act = a.compute_group_action().await.unwrap();
        assert!(matches!(act, GroupAction::Noop));
        p.display();

        println!("2. two node joined");
        let mut nodes = p.nodes();
        nodes.extend_from_slice(&[
            NodeDesc {
                id: 2,
                addr: "".into(),
                capacity: Some(NodeCapacity {
                    cpu_nums: 4.0,
                    replica_count: 0,
                    leader_count: 0,
                }),
            },
            NodeDesc {
                id: 3,
                addr: "".into(),
                capacity: Some(NodeCapacity {
                    cpu_nums: 4.0,
                    replica_count: 0,
                    leader_count: 0,
                }),
            },
        ]);
        p.set_nodes(nodes);
        p.display();

        println!("3. group0 be repaired");
        p.set_groups(vec![GroupDesc {
            id: 1,
            epoch: 0,
            shards: vec![],
            replicas: vec![
                ReplicaDesc {
                    id: 1,
                    node_id: 1,
                    role: ReplicaRole::Voter.into(),
                },
                ReplicaDesc {
                    id: 2,
                    node_id: 2,
                    role: ReplicaRole::Voter.into(),
                },
                ReplicaDesc {
                    id: 3,
                    node_id: 3,
                    role: ReplicaRole::Voter.into(),
                },
            ],
        }]);
        p.display();

        let mut group_id_gen = 2;
        let mut replica_id_gen = 3;

        let act = a.compute_group_action().await.unwrap();
        match act {
            GroupAction::Add(n) => {
                assert!(matches!(n, REPLICA_PER_GROUP));
                for _ in 0..n {
                    let nodes = a
                        .allocate_group_replica(vec![], REPLICA_PER_GROUP)
                        .await
                        .unwrap();
                    println!(
                        "alloc group {} in {:?}",
                        group_id_gen,
                        nodes.iter().map(|n| n.id).collect::<Vec<u64>>()
                    );
                    let mut groups = p.groups();
                    let mut replicas = Vec::new();
                    for n in nodes {
                        replicas.push(ReplicaDesc {
                            id: replica_id_gen,
                            node_id: n.id,
                            role: ReplicaRole::Voter.into(),
                        });
                        replica_id_gen += 1;
                    }
                    groups.push(GroupDesc {
                        id: group_id_gen,
                        epoch: 0,
                        shards: vec![],
                        replicas,
                    });
                    p.set_groups(groups);
                    group_id_gen += 1;
                }
            }
            _ => unreachable!(),
        }
        println!("4. group alloc works and group & replicas balanced");
        let act = a.compute_group_action().await.unwrap();
        assert!(matches!(act, GroupAction::Noop));
        let ract = a.compute_replica_action().await.unwrap();
        assert!(ract.is_empty());
        p.display();

        println!("5. assign shard in groups");
        let cg = a.place_group_for_shard(9).await.unwrap();
        for id in 0..9 {
            let group = cg.get(id % cg.len()).unwrap();
            p.assign_shard(group.id);
            println!(
                "assign shard to group {}, prev_shard_cnt: {}",
                group.id,
                group.shards.len()
            );
        }

        println!("6. node 4 joined");
        let mut nodes = p.nodes();
        nodes.extend_from_slice(&[NodeDesc {
            id: 4,
            addr: "".into(),
            capacity: Some(NodeCapacity {
                cpu_nums: 4.0,
                replica_count: 0,
                leader_count: 0,
            }),
        }]);
        p.set_nodes(nodes);
        p.display();

        println!("7. balance group for new node");
        let act = a.compute_group_action().await.unwrap();
        match act {
            GroupAction::Add(n) => {
                assert!(matches!(n, 2));
                for _ in 0..n {
                    let nodes = a
                        .allocate_group_replica(vec![], REPLICA_PER_GROUP)
                        .await
                        .unwrap();
                    println!(
                        "alloc group {} in {:?}",
                        group_id_gen,
                        nodes.iter().map(|n| n.id).collect::<Vec<u64>>()
                    );
                    let mut groups = p.groups();
                    let mut replicas = Vec::new();
                    for n in nodes {
                        replicas.push(ReplicaDesc {
                            id: replica_id_gen,
                            node_id: n.id,
                            role: ReplicaRole::Voter.into(),
                        });
                        replica_id_gen += 1;
                    }
                    groups.push(GroupDesc {
                        id: group_id_gen,
                        epoch: 0,
                        shards: vec![],
                        replicas,
                    });
                    p.set_groups(groups);
                    group_id_gen += 1;
                }
            }
            _ => unreachable!(),
        }

        // cluster group balanced.
        let act = a.compute_group_action().await.unwrap();
        assert!(matches!(act, GroupAction::Noop));
        p.display();

        println!("8. balance replica between nodes");
        let racts = a.compute_replica_action().await.unwrap();
        assert!(!racts.is_empty());
        for act in &racts {
            match act {
                ReplicaAction::Migrate(ReallocateReplica {
                    group,
                    source_node: _,
                    source_replica,
                    target_node,
                }) => {
                    println!(
                        "move group {} replica {} to {}",
                        group, source_replica, target_node.id
                    );
                    p.move_replica(*source_replica, target_node.id)
                }
                ReplicaAction::Noop => unreachable!(),
            }
        }
        let racts = a.compute_replica_action().await.unwrap();
        assert!(!racts.is_empty());
        for act in &racts {
            match act {
                ReplicaAction::Migrate(ReallocateReplica {
                    group,
                    source_node: _,
                    source_replica,
                    target_node,
                }) => {
                    println!(
                        "move group {} replica {} to {}",
                        group, source_replica, target_node.id
                    );
                    p.move_replica(*source_replica, target_node.id)
                }
                ReplicaAction::Noop => unreachable!(),
            }
        }
        let racts = a.compute_replica_action().await.unwrap();
        assert!(racts.is_empty());
        p.display();

        println!("9. balance shards between groups");
        let sact = a.compute_shard_action().await.unwrap();
        assert!(!sact.is_empty());
        for act in &sact {
            match act {
                ShardAction::Migrate(ReallocateShard {
                    shard,
                    source_group,
                    target_group,
                }) => {
                    println!(
                        "move shard {} from {} to {}",
                        shard, source_group, target_group
                    );
                    p.move_shards(
                        source_group.to_owned(),
                        target_group.to_owned(),
                        shard.to_owned(),
                    );
                }
                ShardAction::Noop => unreachable!(),
            }
        }
        let sact = a.compute_shard_action().await.unwrap();
        assert!(!sact.is_empty());
        for act in &sact {
            match act {
                ShardAction::Migrate(ReallocateShard {
                    shard,
                    source_group,
                    target_group,
                }) => {
                    println!(
                        "move shard {} from {} to {}",
                        shard, source_group, target_group
                    );
                    p.move_shards(
                        source_group.to_owned(),
                        target_group.to_owned(),
                        shard.to_owned(),
                    );
                }
                ShardAction::Noop => unreachable!(),
            }
        }
        let sact = a.compute_shard_action().await.unwrap();
        assert!(sact.is_empty());
        p.display();

        println!("done");
    });
}

pub struct MockInfoProvider {
    nodes: Arc<Mutex<Vec<NodeDesc>>>,
    groups: Arc<Mutex<GroupInfo>>,
    shard_id_gen: AtomicU64,
}

#[derive(Default)]
struct GroupInfo {
    descs: Vec<GroupDesc>,
    node_replicas: HashMap<u64, Vec<(ReplicaDesc, u64)>>,
}

impl MockInfoProvider {
    pub fn new() -> Self {
        Self {
            nodes: Default::default(),
            groups: Default::default(),
            shard_id_gen: AtomicU64::new(1),
        }
    }
}

#[crate::async_trait]
impl AllocSource for MockInfoProvider {
    async fn refresh_all(&self) -> Result<()> {
        Ok(())
    }

    fn nodes(&self) -> Vec<NodeDesc> {
        let nodes = self.nodes.lock().unwrap();
        nodes.to_owned()
    }

    fn groups(&self) -> Vec<GroupDesc> {
        let groups = self.groups.lock().unwrap();
        groups.descs.to_owned()
    }

    fn node_replicas(&self, node_id: &u64) -> Vec<(ReplicaDesc, u64)> {
        let groups = self.groups.lock().unwrap();
        groups
            .node_replicas
            .get(node_id)
            .map(ToOwned::to_owned)
            .unwrap_or_default()
    }
}

impl MockInfoProvider {
    fn set_nodes(&self, ns: Vec<NodeDesc>) {
        let mut nodes = self.nodes.lock().unwrap();
        _ = std::mem::replace(&mut *nodes, ns);
    }

    fn set_groups(&self, gs: Vec<GroupDesc>) {
        let mut groups = self.groups.lock().unwrap();
        let mut node_replicas: HashMap<u64, Vec<(ReplicaDesc, u64)>> = HashMap::new();
        for group in gs.iter() {
            for replica in &group.replicas {
                match node_replicas.entry(replica.node_id) {
                    Entry::Occupied(mut ent) => {
                        (*ent.get_mut()).push((replica.to_owned(), group.id.to_owned()));
                    }
                    Entry::Vacant(ent) => {
                        ent.insert(vec![(replica.to_owned(), group.id.to_owned())]);
                    }
                };
            }
        }

        // test only fix node.replica logic
        let mut nodes = self.nodes();
        for n in nodes.iter_mut() {
            let mut cap = n.capacity.take().unwrap();
            cap.replica_count = node_replicas.get(&n.id).unwrap().len() as u64;
            n.capacity = Some(cap)
        }
        self.set_nodes(nodes);

        _ = std::mem::replace(
            &mut *groups,
            GroupInfo {
                descs: gs,
                node_replicas,
            },
        );
    }

    pub fn move_replica(&self, replica_id: u64, node: u64) {
        let mut groups = self.groups();
        for group in groups.iter_mut() {
            for replica in group.replicas.iter_mut() {
                if replica.id == replica_id {
                    replica.node_id = node;
                    break;
                }
            }
        }
        self.set_groups(groups);
    }

    pub fn move_shards(&self, sgroup: u64, tgroup: u64, shard: u64) {
        let mut groups = self.groups();

        let mut shard_desc = None;
        for group in groups.iter_mut() {
            if group.id == sgroup {
                group.shards.retain(|s| {
                    if s.id == shard {
                        shard_desc = Some(s.to_owned())
                    }
                    s.id != shard
                });
                break;
            }
        }

        if let Some(shard_desc) = shard_desc {
            for group in groups.iter_mut() {
                if group.id == tgroup {
                    group.shards.push(shard_desc);
                    break;
                }
            }
        }

        self.set_groups(groups);
    }

    pub fn assign_shard(&self, group_id: u64) {
        let mut groups = self.groups();
        for group in groups.iter_mut() {
            if group.id == group_id {
                let s = ShardDesc {
                    id: self.shard_id_gen.fetch_add(1, Ordering::Relaxed),
                    ..Default::default()
                };
                group.shards.push(s);
            }
        }
        self.set_groups(groups);
    }

    pub fn display(&self) {
        let groups = self.groups.lock().unwrap();
        println!("----------");
        for (n, g) in &groups.node_replicas {
            println!(
                "node replicas: {} -> {:?}",
                n,
                g.iter().map(|r| r.0.id).collect::<Vec<u64>>()
            )
        }

        for g in &groups.descs {
            let shards = g.shards.iter().map(|s| s.id).collect::<Vec<u64>>();
            println!("group shards: {} -> {:?}", g.id, shards);
        }

        let nodes = self.nodes.lock().unwrap();
        println!(
            "cluster_nodes: {:?}",
            nodes.iter().map(|n| n.id).collect::<Vec<u64>>()
        );
        println!("----------");
    }
}