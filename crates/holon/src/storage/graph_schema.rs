//! Converts entity type definitions and module contributions into a GQL `GraphSchema`.
//!
//! This module bridges `holon-api` types (`TypeDefinition`, `GraphNodeDef`,
//! `GraphEdgeDef`) with `gql_transform::resolver` types. The macro crate cannot depend
//! on `gql_transform`, so this conversion lives here in the `holon` crate.

use std::collections::HashMap;

use gql_transform::resolver::{
    ColumnMapping, EavEdgeResolver, EavNodeResolver, EdgeDef, ForeignKeyEdgeResolver, GraphSchema,
    JoinTableEdgeResolver, MappedNodeResolver, NodeResolver,
};
use holon_api::entity::{GraphEdgeDef, GraphNodeDef, TypeDefinition};

use super::schema_module::EdgeFieldDescriptor;

/// Collects entity type definitions and module contributions, then builds a `GraphSchema`.
///
/// Clone is supported so that `build()` (which consumes `self`) can be called
/// on a snapshot while the original registry remains available for future mutations.
#[derive(Clone)]
pub struct GraphSchemaRegistry {
    type_defs: Vec<TypeDefinition>,
    extra_nodes: Vec<GraphNodeDef>,
    extra_edges: Vec<GraphEdgeDef>,
    edge_fields: Vec<EdgeFieldDescriptor>,
}

impl GraphSchemaRegistry {
    pub fn new() -> Self {
        Self {
            type_defs: Vec::new(),
            extra_nodes: Vec::new(),
            extra_edges: Vec::new(),
            edge_fields: Vec::new(),
        }
    }

    /// Register an entity type definition.
    /// Only types with `graph_label` will produce GQL nodes.
    pub fn register_type(&mut self, type_def: TypeDefinition) {
        self.type_defs.push(type_def);
    }

    /// Register additional graph nodes from a SchemaModule.
    pub fn register_nodes(&mut self, nodes: Vec<GraphNodeDef>) {
        self.extra_nodes.extend(nodes);
    }

    /// Register additional graph edges from a SchemaModule.
    pub fn register_edges(&mut self, edges: Vec<GraphEdgeDef>) {
        self.extra_edges.extend(edges);
    }

    /// Register edge-typed fields from a SchemaModule.
    ///
    /// Each descriptor wires a `JoinTableEdgeResolver` so GQL
    /// `MATCH (a)-[:edge]->(b)` patterns dispatch to a JOIN against the
    /// junction table — distinct from `register_edges`, which always wires
    /// `ForeignKeyEdgeResolver`.
    pub fn register_edge_fields(&mut self, edge_fields: Vec<EdgeFieldDescriptor>) {
        self.edge_fields.extend(edge_fields);
    }

    /// Build the final `GraphSchema` from all registered types and contributions.
    pub fn build(self) -> GraphSchema {
        let mut nodes: HashMap<String, Box<dyn NodeResolver>> = HashMap::new();
        let mut edges: HashMap<String, EdgeDef> = HashMap::new();

        // Build a map from entity name → (TypeDefinition, graph_label)
        // so we can resolve reference targets when building edges.
        let entity_info: HashMap<String, (&TypeDefinition, &str)> = self
            .type_defs
            .iter()
            .filter_map(|td| {
                td.graph_label
                    .as_ref()
                    .map(|label| (td.name.clone(), (td, label.as_str())))
            })
            .collect();

        for td in &self.type_defs {
            let Some(ref label) = td.graph_label else {
                continue;
            };

            let columns: Vec<ColumnMapping> = td
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
                    table_name: td.name.clone(),
                    id_col: td.primary_key.clone(),
                    label: label.clone(),
                    columns,
                }),
            );

            // Build edges from fields with edge_name + reference_target
            for field in &td.fields {
                let Some(ref edge_name) = field.edge_name else {
                    continue;
                };
                let Some(ref target_entity_name) = field.reference_target else {
                    continue;
                };
                let target_label = entity_info
                    .get(target_entity_name.as_str())
                    .map(|(_, lbl)| (*lbl).to_string());
                let (target_table, target_id) = entity_info
                    .get(target_entity_name.as_str())
                    .map(|(td, _)| (td.name.clone(), td.primary_key.clone()))
                    .unwrap_or_else(|| (target_entity_name.clone(), "id".into()));

                edges.insert(
                    edge_name.clone(),
                    EdgeDef {
                        source_label: Some(label.clone()),
                        target_label,
                        resolver: Box::new(ForeignKeyEdgeResolver {
                            fk_table: td.name.clone(),
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

        // Register edge-typed fields (junction-table edges). Source/target
        // labels are looked up from the entity registry by entity name.
        for descriptor in self.edge_fields {
            let source_label = entity_info
                .get(descriptor.entity.as_str())
                .map(|(_, lbl)| (*lbl).to_string());
            edges.insert(
                descriptor.field.clone(),
                EdgeDef {
                    source_label,
                    target_label: None,
                    resolver: Box::new(JoinTableEdgeResolver {
                        join_table: descriptor.join_table,
                        source_column: descriptor.source_col,
                        target_column: descriptor.target_col,
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
    use holon_api::FieldSchema;

    fn block_type_def() -> TypeDefinition {
        TypeDefinition {
            name: "block".into(),
            primary_key: "id".into(),
            graph_label: Some("block".into()),
            fields: vec![
                FieldSchema::new("id", "TEXT").primary_key().indexed(),
                FieldSchema::new("content", "TEXT"),
            ],
            ..TypeDefinition::from_table_name("block")
        }
    }

    fn block_type_def_with_edge() -> TypeDefinition {
        TypeDefinition {
            name: "block".into(),
            primary_key: "id".into(),
            graph_label: Some("block".into()),
            fields: vec![
                FieldSchema::new("id", "TEXT").primary_key().indexed(),
                FieldSchema::new("parent_id", "TEXT")
                    .indexed()
                    .reference_target("block")
                    .edge_name("CHILD_OF"),
            ],
            ..TypeDefinition::from_table_name("block")
        }
    }

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
        registry.register_type(block_type_def());

        let schema = registry.build();
        assert!(schema.nodes.contains_key("block"));
        assert_eq!(schema.nodes.len(), 1);
    }

    #[test]
    fn entity_without_graph_label_skipped() {
        let mut registry = GraphSchemaRegistry::new();
        registry.register_type(TypeDefinition::from_table_name("internal_thing"));

        let schema = registry.build();
        assert!(schema.nodes.is_empty());
    }

    #[test]
    fn reference_field_with_edge_produces_edge() {
        let mut registry = GraphSchemaRegistry::new();
        registry.register_type(block_type_def_with_edge());

        let schema = registry.build();
        assert!(schema.edges.contains_key("CHILD_OF"));
        let edge = &schema.edges["CHILD_OF"];
        assert_eq!(edge.source_label.as_deref(), Some("block"));
        assert_eq!(edge.target_label.as_deref(), Some("block"));
    }

    #[test]
    fn extra_node_def_registered() {
        let mut registry = GraphSchemaRegistry::new();
        registry.register_nodes(vec![GraphNodeDef {
            label: "focus_root".into(),
            table_name: "focus_roots".into(),
            id_column: "root_id".into(),
            columns: vec![
                ("region".into(), "region".into()),
                ("root_id".into(), "root_id".into()),
            ],
        }]);

        let schema = registry.build();
        assert!(schema.nodes.contains_key("focus_root"));
    }

    #[test]
    fn extra_edge_def_registered() {
        let mut registry = GraphSchemaRegistry::new();
        registry.register_edges(vec![GraphEdgeDef {
            edge_name: "FOCUSES_ON".into(),
            source_label: Some("current_focus".into()),
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
