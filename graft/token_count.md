# token_count.go

- countOpenAIPayloadTokens · function · L11-L23 — func countOpenAIPayloadTokens(model string, payload map[string]any) (int64, error)
- tokenizerForModel · function · L25-L49 — func tokenizerForModel(model string) (tokenizer.Codec, error)
- countOpenAIChatTokens · function · L51-L83 — func countOpenAIChatTokens(encoder tokenizer.Codec, payload []byte) (int64, error)
- collectOpenAIMessagesForCount · function · L85-L97 — func collectOpenAIMessagesForCount(messages []any, segments *[]string)
- collectOpenAIContentForCount · function · L99-L135 — func collectOpenAIContentForCount(content any, segments *[]string)
- collectOpenAIToolCallsForCount · function · L137-L155 — func collectOpenAIToolCallsForCount(toolCalls []any, segments *[]string)
- collectOpenAIToolsForCount · function · L157-L182 — func collectOpenAIToolsForCount(rawTools any, segments *[]string)
- collectOpenAIToolChoiceForCount · function · L184-L193 — func collectOpenAIToolChoiceForCount(toolChoice any, segments *[]string)
- collectOpenAIResponseFormatForCount · function · L195-L209 — func collectOpenAIResponseFormatForCount(responseFormat any, segments *[]string)
- addSegment · function · L211-L218 — func addSegment(segments *[]string, value string)
