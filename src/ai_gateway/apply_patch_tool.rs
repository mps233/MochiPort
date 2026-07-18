pub const APPLY_PATCH_TOOL_NAME: &str = "apply_patch";
pub const APPLY_PATCH_INPUT_DESCRIPTION: &str = "The entire apply_patch patch body.";

pub const APPLY_PATCH_DESCRIPTION: &str = "Apply file changes using a structured patch format. This is ideal for multi-file or multi-hunk edits where shell-based file writes would be brittle.\n\
The patch language is a stripped-down, file-oriented diff format designed to be easy to parse and safe to apply. A patch must use this envelope:\n\n\
*** Begin Patch\n\
[ one or more file sections ]\n\
*** End Patch\n\n\
Within that envelope, each file operation starts with one of these headers:\n\n\
*** Add File: <path> - create a new file. Every following line is a + line.\n\
*** Delete File: <path> - remove an existing file. Nothing follows.\n\
*** Update File: <path> - patch an existing file in place, optionally followed by *** Move to: <new path>.\n\n\
Update hunks start with @@ and contain lines prefixed with a space, -, or +.\n\
Use *** Move to: <new path> after an *** Update File header to rename or move a file.\n\
Use *** End of File after update lines when you need an EOF-only insertion.\n\
Prefer workspace-relative paths. Absolute paths are accepted by Codex, but use them only when the user explicitly asks to edit that exact absolute path and permissions allow it.\n\n\
Call this tool with JSON arguments matching {\"input\":\"<the entire patch body>\"}. The `input` value must contain only the patch body and its final non-whitespace line must be exactly `*** End Patch`.\n\n\
Important rules:\n\
- For *** Add File, every content line must start with +. Blank lines must be written as +.\n\
- Do not write raw file content after an Add File header.\n\
- For *** Update File, context lines start with one space, removed lines with -, and added lines with +.\n\
- If apply_patch returns a format error, fix the patch and retry apply_patch before using shell commands to write files.\n\n\
Few-shot examples:\n\n\
Add a markdown file:\n\
*** Begin Patch\n\
*** Add File: notes.md\n\
+# Notes\n\
+\n\
+- First item\n\
+- Second item\n\
*** End Patch\n\n\
Update a file:\n\
*** Begin Patch\n\
*** Update File: src/example.txt\n\
@@\n\
 old line kept as context\n\
-old value\n\
+new value\n\
*** End Patch\n\n\
Move and update a file:\n\
*** Begin Patch\n\
*** Update File: old/path.txt\n\
*** Move to: new/path.txt\n\
@@\n\
-old name\n\
+new name\n\
*** End Patch\n\n\
Delete a file:\n\
*** Begin Patch\n\
*** Delete File: old.txt\n\
*** End Patch\n\n\
Update multiple files in one patch:\n\
*** Begin Patch\n\
*** Update File: src/lib.rs\n\
@@\n\
-pub mod old;\n\
+pub mod new;\n\
*** Update File: README.md\n\
@@\n\
-Old heading\n\
+New heading\n\
*** End Patch\n\n\
Insert at end of file:\n\
*** Begin Patch\n\
*** Update File: notes.md\n\
@@\n\
 existing final line\n\
*** End of File\n\
+appended line\n\
*** End Patch";

pub fn apply_patch_description_for_argument(argument_name: &str) -> String {
    if argument_name == "input" {
        return APPLY_PATCH_DESCRIPTION.to_string();
    }
    APPLY_PATCH_DESCRIPTION
        .replace(
            "{\"input\":\"<the entire patch body>\"}",
            &format!("{{\"{argument_name}\":\"<the entire patch body>\"}}"),
        )
        .replace(
            "The `input` value must contain only the patch body",
            &format!("The `{argument_name}` value must contain only the patch body"),
        )
}
