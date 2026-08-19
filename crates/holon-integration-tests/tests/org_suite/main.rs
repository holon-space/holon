//! Org parse / render / write-back and vault-ingest tests.

mod bidirectional_sync;
mod convert_block_to_page_materializes_child_file;
mod doc_id_writeback_stability;
mod idless_external_reedit_dup;
mod journals_seed_file_collision;
mod local_vault_fidelity;
mod logseq_org_vault_ingest;
mod multiline_serializer_test;
mod org_roundtrip_pbt;
mod org_serializer_prod_content_parity;
mod probe_link_marks_roundtrip;
mod writeback_stale_cross_doc_prune;
