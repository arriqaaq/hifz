; Symbol extraction queries for Rust.
; Each pattern captures @name (the identifier) and @def.<kind> (the whole defining node).

(function_item name: (identifier) @name) @def.function
(struct_item name: (type_identifier) @name) @def.struct
(enum_item name: (type_identifier) @name) @def.enum
(trait_item name: (type_identifier) @name) @def.trait
(union_item name: (type_identifier) @name) @def.struct
(type_item name: (type_identifier) @name) @def.type
(mod_item name: (identifier) @name) @def.module
(const_item name: (identifier) @name) @def.const
(static_item name: (identifier) @name) @def.const
(macro_definition name: (identifier) @name) @def.macro
