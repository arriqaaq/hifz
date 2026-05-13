; Symbol extraction queries for Go.

(function_declaration name: (identifier)        @name) @def.function
(method_declaration   name: (field_identifier)  @name) @def.method
(type_spec            name: (type_identifier)   @name) @def.type
