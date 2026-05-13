; Symbol extraction queries for C++.

(function_definition
  declarator: (function_declarator declarator: (identifier) @name)) @def.function
(function_definition
  declarator: (function_declarator declarator: (qualified_identifier) @name)) @def.function
(class_specifier name: (type_identifier) @name) @def.class
(struct_specifier
  name: (type_identifier) @name
  body: (field_declaration_list)) @def.struct
(namespace_definition name: (namespace_identifier) @name) @def.namespace
