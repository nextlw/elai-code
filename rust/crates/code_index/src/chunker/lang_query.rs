//! Tree-sitter queries per language. Capture groups: @symbol (name) on the parent declaration node.

pub const RUST_QUERY: &str = r"
[
  (function_item name: (identifier) @symbol) @decl
  (impl_item) @decl
  (struct_item name: (type_identifier) @symbol) @decl
  (enum_item name: (type_identifier) @symbol) @decl
  (trait_item name: (type_identifier) @symbol) @decl
]
";

pub const TYPESCRIPT_QUERY: &str = r"
[
  (function_declaration name: (identifier) @symbol) @decl
  (method_definition name: (property_identifier) @symbol) @decl
  (class_declaration name: (type_identifier) @symbol) @decl
  (interface_declaration name: (type_identifier) @symbol) @decl
  (enum_declaration name: (identifier) @symbol) @decl
]
";

pub const JAVASCRIPT_QUERY: &str = r"
[
  (function_declaration name: (identifier) @symbol) @decl
  (method_definition name: (property_identifier) @symbol) @decl
  (class_declaration name: (identifier) @symbol) @decl
]
";

pub const PYTHON_QUERY: &str = r"
[
  (function_definition name: (identifier) @symbol) @decl
  (class_definition name: (identifier) @symbol) @decl
]
";

pub const GO_QUERY: &str = r"
[
  (function_declaration name: (identifier) @symbol) @decl
  (method_declaration name: (field_identifier) @symbol) @decl
  (type_declaration (type_spec name: (type_identifier) @symbol)) @decl
]
";
