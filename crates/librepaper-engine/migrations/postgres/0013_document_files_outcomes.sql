-- Agents create and delete text files through document_files, and its
-- outcomes are recorded beside the other writing tools'.
ALTER TABLE operation_outcomes DROP CONSTRAINT operation_outcomes_tool_check;
ALTER TABLE operation_outcomes
    ADD CONSTRAINT operation_outcomes_tool_check CHECK (
        tool IN ('document_propose', 'document_apply', 'document_comment', 'document_files')
    );
