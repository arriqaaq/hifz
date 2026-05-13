; Symbol extraction queries for C.

(function_definition
  declarator: (function_declarator declarator: (identifier) @name)) @def.function
(struct_specifier
  name: (type_identifier) @name
  body: (field_declaration_list)) @def.struct
(enum_specifier name: (type_identifier) @name) @def.enum
(type_definition declarator: (type_identifier) @name) @def.type
