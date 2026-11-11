#[cfg(test)]
mod review_stream_repros {
    use super::*;
    #[test]
    fn review_incomplete_stream_can_finish_with_executable_tool() {
        let mut builder=StreamingMessageBuilder::default();
        builder.apply_event(r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"apply_patch","input":{"patch":"valid but unconfirmed final block"}}}"#,&mut |_|{}).unwrap();
        let message=builder.finish().unwrap();
        assert!(matches!(message.content[0],ContentBlock::ToolUse{..}));
    }
    #[test]
    fn review_token_limit_is_discarded_by_stream_builder() {
        let mut builder=StreamingMessageBuilder::default();
        builder.apply_event(r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":"partial"}}"#,&mut |_|{}).unwrap();
        builder.apply_event(r#"{"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":1000}}"#,&mut |_|{}).unwrap();
        builder.apply_event(r#"{"type":"message_stop"}"#,&mut |_|{}).unwrap();
        let message=builder.finish().unwrap();
        assert!(matches!(message.content[0],ContentBlock::Text{..}));
    }
}
