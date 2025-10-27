//! Converts entity schema metadata and module contributions into a GQL `GraphSchema`.
//!
//! This module bridges `holon-api` intermediate types (`EntitySchema`, `GraphNodeDef`,
//! `GraphEdgeDef`) with `gql_transform::resolver` types. The macro crate cannot depend
//! on `gql_transform`, so this conversion lives here in the `holon` crate.

use std::collections::HashMap;

use gql_transform::resolver::{
    ColumnMapping, EavEdgeResolver, EavNodeResolver, EdgeDef, ForeignKeyEdgeResolver, GraphSchema,
    MappedNodeResolver, NodeResolver,
};
use holon_api::FieldType;
#[cfg(test)]
use holon_api::entity::EntityFieldSchema;
use holon_api::entity::{EntitySchema, GraphEdgeDef, GraphNodeDef};

/// Collects entity schemas and module contributions, then builds a `GraphSchema`.
///
/// Clone is supported so that `build()` (which consumes `self`) can be called
/// on a snapshot while the original registry remains available for future mutations.
#[derive(Clone)]
pub struct GraphSchemaRegistry {
    entity_schemas: Vec<EntitySchema>,
    extra_nodes: Vec<GraphNodeDef>,
    extra_edges: Vec<GraphEdgeDef>,
}

impl GraphSchemaRegistry {
    pub fn new() -> Self {
        Self {
            entity_schemas: Vec::new(),
            extra_nodes: Vec::new(),
            extra_edges: Vec::new(),
        }
    }

    /// Register an entity schema (from `T::entity_schema()`).
    /// Only entities with `graph_label` will produce GQL nodes.
    pub fn register_entity(&mut self, schema: EntitySchema) {
        self.entity_schemas.push(schema);
    }

    /// Register additional graph nodes from a SchemaModule.
    pub fn register_nodes(&mut self, nodes: Vec<GraphNodeDef>) {
        self.extra_nodes.extend(nodes);
    }

    /// Register additional graph edges from a SchemaModule.
    pub fn register_edges(&mut self, edges: Vec<GraphEdgeDef>) {
        self.extra_edges.extend(edges);
    }

    /// Build the final `GraphSchema` from all registered entities and contributions.
    pub fn build(self) -> GraphSchema {
        let mut nodes: HashMap<String, Box<dyn NodeResolver>> = HashMap::new();
        let mut edges: HashMap<String, EdgeDef> = HashMap::new();

        // Build a map from entity name → (table_name, primary_key, graph_label)
        // so we can resolve reference targets when building edges.
        let entity_info: HashMap<String, (&EntitySchema, &str)> = self
            .entity_schemas
            .iter()
            .filter_map(|schema| {
                schema
                    .graph_label
                    .as_ref()
                    .map(|label| (schema.name.clone(), (schema, label.as_str())))
            })
            .collect();

        for schema in &self.entity_schemas {
            let Some(ref label) = schema.graph_label else {
                continue;
            };

            let columns: Vec<ColumnMapping> = schema
                .fields
                .iter()
                .map(|f| ColumnMapping {
                    property_name: f.name.clone(),
                    column_name: f.name.clone(),
                })
                .collect();

            nodes.insert(
                label.clone(),
                Box::new(MappedNodeResolver {
                    table_name: schema.name.clone(),
                    id_col: schema.primary_key.clone(),
                    label: label.clone(),
                    columns,
                }),
            );

            // Build edges from #[reference(..., edge = "...")] fields
            for field in &schema.fields {
                let Some(ref edge_name) = field.edge_name else {
                    continue;
                };
                let target_entity_name = match &field.field_type {
                    FieldType::Reference(name) => name,
                    _ => continue,
                };
                let target_label = entity_info
                    .get(target_entity_name)
                    .map(|(_, lbl)| (*lbl).to_string());
                let (target_table, target_id) = entity_info
                    .get(target_entity_name)
                    .map(|(s, _)| (s.name.clone(), s.primary_key.clone()))
                    .unwrap_or_else(|| (target_entity_name.clone(), "id".into()));

                edges.insert(
                    edge_name.clone(),
                    EdgeDef {
                        source_label: Some(label.clone()),
                        target_label,
                        resolver: Box::new(ForeignKeyEdgeResolver {
                            fk_table: schema.name.clone(),
                            fk_column: field.name.clone(),
                            target_table,
                            target_id_column: target_id,
                        }),
                    },
                );
            }
        }

        // Register extra nodes from SchemaModule contributions
        for node_def in self.extra_nodes {
            let columns: Vec<ColumnMapping> = node_def
                .columns
                .into_iter()
                .map(|(prop, col)| ColumnMapping {
                    property_name: prop,
                    column_name: col,
                })
                .collect();

            nodes.insert(
                node_def.label.clone(),
                Box::new(MappedNodeResolver {
                    table_name: node_def.table_name,
                    id_col: node_def.id_column,
                    label: node_def.label,
                    columns,
                }),
            );
        }

        // Register extra edges from SchemaModule contributions
        for edge_def in self.extra_edges {
            edges.insert(
                edge_def.edge_name.clone(),
                EdgeDef {
                    source_label: edge_def.source_label,
                    target_label: edge_def.target_label,
                    resolver: Box::new(ForeignKeyEdgeResolver {
                        fk_table: edge_def.fk_table,
                        fk_column: edge_def.fk_column,
                        target_table: edge_def.target_table,
                        target_id_column: edge_def.target_id_column,
                    }),
                },
            );
        }

        GraphSchema {
            nodes,
            edges,
            default_node_resolver: Box::new(EavNodeResolver),
            default_edge_resolver: Box::new(EavEdgeResolver),
            raw_return: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_registry_builds_with_eav_defaults() {
        let registry = GraphSchemaRegistry::new();
        let schema = registry.build();
        assert!(schema.nodes.is_empty());
        assert!(schema.edges.is_empty());
    }

    #[test]
    fn entity_with_graph_label_produces_node() {
        let mut registry = GraphSchemaRegistry::new();
        registry.register_entity(EntitySchema {
            name: "block".into(),
            primary_key: "id".into(),
            graph_label: Some("Block".into()),
            fields: vec![
                EntityFieldSchema {
                    name: "id".into(),
                    field_type: FieldType::String,
                    required: true,
                    indexed: true,
                    edge_name: None,
                },
                EntityFieldSchema {
                    name: "content".into(),
                    field_type: FieldType::String,
                    required: true,
                    indexed: false,
                    edge_name: None,
                },
            ],
        });

        let schema = registry.build();
        assert!(schema.nodes.contains_key("Block"));
        assert_eq!(schema.nodes.len(), 1);
    }

    #[test]
    fn entity_without_graph_label_skipped() {
        let mut registry = GraphSchemaRegistry::new();
        registry.register_entity(EntitySchema {
            name: "internal_thing".into(),
            primary_key: "id".into(),
            graph_label: None,
            fields: vec![],
        });

        let schema = registry.build();
        assert!(schema.nodes.is_empty());
    }

    #[test]
    fn reference_field_with_edge_produces_edge() {
        let mut registry = GraphSchemaRegistry::new();
        registry.register_entity(EntitySchema {
            name: "block".into(),
            primary_key: "id".into(),
            graph_label: Some("Block".into()),
            fields: vec![
                EntityFieldSchema {
                    name: "id".into(),
                    field_type: FieldType::String,
                    required: true,
                    indexed: true,
                    edge_name: None,
                },
                EntityFieldSchema {
                    name: "parent_id".into(),
                    field_type: FieldType::Reference("block".into()),
                    required: true,
                    indexed: true,
                    edge_name: Some("CHILD_OF".into()),
                },
            ],
        });

        let schema = registry.build();
        assert!(schema.edges.contains_key("CHILD_OF"));
        let edge = &schema.edges["CHILD_OF"];
        assert_eq!(edge.source_label.as_deref(), Some("Block"));
        assert_eq!(edge.target_label.as_deref(), Some("Block"));
    }

    #[test]
    fn extra_node_def_registered() {
        let mut registry = GraphSchemaRegistry::new();
        registry.register_nodes(vec![GraphNodeDef {
            label: "FocusRoot".into(),
            table_name: "focus_roots".into(),
            id_column: "root_id".into(),
            columns: vec![
                ("region".into(), "region".into()),
                ("root_id".into(), "root_id".into()),
            ],
        }]);

        let schema = registry.build();
        assert!(schema.nodes.contains_key("FocusRoot"));
    }

    #[test]
    fn extra_edge_def_registered() {
        let mut registry = GraphSchemaRegistry::new();
        registry.register_edges(vec![GraphEdgeDef {
            edge_name: "FOCUSES_ON".into(),
            source_label: Some("CurrentFocus".into()),
            target_label: Some("Block".into()),
            fk_table: "current_focus".into(),
            fk_column: "block_id".into(),
            target_table: "block".into(),
            target_id_column: "id".into(),
        }]);

        let schema = registry.build();
        assert!(schema.edges.contains_key("FOCUSES_ON"));
    }
}
