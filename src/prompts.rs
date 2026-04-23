/// System prompt for LLM-powered observation compression.
pub const COMPRESSION_SYSTEM: &str = r#"You compress raw agent tool-use observations into structured memory records.
Given a raw observation (JSON), output ONLY an XML block:

<observation>
  <type>file_read|file_write|file_edit|command_run|search|web_fetch|conversation|error|decision|discovery|subagent|notification|task|other</type>
  <title>Short descriptive title (max 80 chars)</title>
  <subtitle>Optional one-liner</subtitle>
  <facts><fact>Atomic fact 1</fact><fact>Atomic fact 2</fact></facts>
  <narrative>2-3 sentence summary of what happened and why it matters.</narrative>
  <keywords><keyword>keyword1</keyword><keyword>keyword2</keyword></keywords>
  <files><file>exact/file/path.rs</file></files>
  <importance>1-10 scale (1-3: routine reads, 4-6: edits/commands, 7-9: architectural decisions, 10: breaking changes)</importance>
</observation>"#;

/// System prompt for semantic memory consolidation.
pub const SEMANTIC_MERGE_SYSTEM: &str = r#"You merge session summaries into semantic facts.
Extract the most important, reusable facts from the summaries provided.
Output ONLY XML:

<facts>
  <fact confidence="0.9">Reusable fact about the project</fact>
  <fact confidence="0.7">Another fact</fact>
</facts>"#;

/// System prompt for procedural memory extraction.
pub const PROCEDURAL_EXTRACTION_SYSTEM: &str = r#"You extract recurring workflows from memory patterns.
Given patterns that appear frequently, extract step-by-step procedures.
Output ONLY XML:

<procedures>
  <procedure name="procedure name" trigger="when this should be triggered">
    <step>Step 1 description</step>
    <step>Step 2 description</step>
  </procedure>
</procedures>"#;
