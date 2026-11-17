#include <typebridge/type_bridge.h>

int main(void) {
  const type_bridge_pending_t *pending = NULL;
  const type_bridge_document_t *doc = NULL, *player = NULL;
  const type_bridge_row_t *row = NULL;
  type_bridge_byte_view_t name = {NULL, 0u}, text = {NULL, 0u};
  type_bridge_value_kind_t kind = TYPE_BRIDGE_VALUE_ABSENT;
  size_t count = 0u;
  int64_t integer = 0;
  uint64_t bits = 0u;
  uint8_t boolean = 0u;
  (void)type_bridge_pending_document_count(pending, &count);
  (void)type_bridge_pending_document(pending, 0u, &doc);
  (void)type_bridge_pending_row_count(pending, &count);
  (void)type_bridge_pending_row(pending, 0u, &row);
  (void)type_bridge_document_iid(doc, &text);
  (void)type_bridge_document_type(doc, &text);
  (void)type_bridge_document_role(doc, 0u, &player);
  (void)type_bridge_document_attribute_count(doc, name, &count);
  (void)type_bridge_document_attribute_kind(doc, name, 0u, &kind);
  (void)type_bridge_row_value_kind(row, name, &kind);
#define READ(suffix, output) \
  (void)type_bridge_document_attribute_##suffix(doc, name, 0u, output); \
  (void)type_bridge_row_value_##suffix(row, name, output)
  READ(string, &text);
  READ(long, &integer);
  READ(double, &bits);
  READ(boolean, &boolean);
  READ(date, &text);
  READ(datetime, &text);
  READ(datetime_tz, &text);
  READ(decimal, &text);
  READ(duration, &text);
#undef READ
  return 0;
}
