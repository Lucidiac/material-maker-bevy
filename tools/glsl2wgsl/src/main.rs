use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = env::args().collect();

    let mut input_file: Option<PathBuf> = None;
    let mut output_file: Option<PathBuf> = None;
    let mut bevy_mode = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--bevy" | "-b" => bevy_mode = true,
            "-o" | "--output" => {
                i += 1;
                if i < args.len() {
                    output_file = Some(PathBuf::from(&args[i]));
                }
            }
            "--help" | "-h" => {
                eprintln!("Usage: glsl2wgsl [OPTIONS] [INPUT_FILE]");
                eprintln!();
                eprintln!("Convert Material Maker GLSL fragment shaders to WGSL.");
                eprintln!("If no input file is given, reads from stdin.");
                eprintln!();
                eprintln!("Options:");
                eprintln!("  -o, --output FILE  Write WGSL to FILE instead of stdout");
                eprintln!("  -b, --bevy         Post-process for Bevy compatibility");
                eprintln!("  -h, --help         Show this help");
                std::process::exit(0);
            }
            arg if !arg.starts_with('-') => {
                input_file = Some(PathBuf::from(arg));
            }
            _ => {
                eprintln!("Unknown option: {}", args[i]);
                std::process::exit(1);
            }
        }
        i += 1;
    }

    let glsl_source = if let Some(path) = &input_file {
        fs::read_to_string(path).unwrap_or_else(|e| {
            eprintln!("Failed to read input file: {}", e);
            std::process::exit(1);
        })
    } else {
        let mut s = String::new();
        io::stdin().read_to_string(&mut s).unwrap_or_else(|e| {
            eprintln!("Failed to read stdin: {}", e);
            std::process::exit(1);
        });
        s
    };

    match convert(&glsl_source, bevy_mode) {
        Ok(wgsl_source) => {
            if let Some(path) = &output_file {
                fs::write(path, &wgsl_source).unwrap_or_else(|e| {
                    eprintln!("Failed to write output file: {}", e);
                    std::process::exit(1);
                });
            } else {
                print!("{}", wgsl_source);
            }
        }
        Err(e) => {
            eprintln!("Error converting GLSL to WGSL:\n{}", e);
            std::process::exit(1);
        }
    }
}

/// Information about a sampler2D texture extracted from the GLSL source.
#[derive(Debug, Clone)]
struct TextureInfo {
    /// The GLSL variable name (e.g. "texture_1")
    name: String,
}

/// Full conversion pipeline:
/// 1. Preprocess GLSL (fix naga compat, extract textures)
/// 2. Convert to WGSL via naga
/// 3. Post-process (re-inject textures, optionally add Bevy boilerplate)
fn convert(glsl_source: &str, bevy_mode: bool) -> Result<String, String> {
    let (preprocessed, textures) = preprocess_glsl(glsl_source);
    let mut wgsl = naga_convert(&preprocessed)?;

    // Re-inject texture declarations and fix sampling calls
    wgsl = postprocess_textures(wgsl, &textures);

    if bevy_mode {
        wgsl = bevy_postprocess(wgsl);
    }

    Ok(wgsl)
}

/// Preprocess Material Maker GLSL to be compatible with naga's GLSL frontend:
///
/// - Remove `precision` qualifiers (not valid in GLSL 450 core)
/// - Strip default values from `uniform` declarations
/// - Convert `const` globals to regular globals (naga doesn't support `const` qualifier)
/// - Extract `uniform sampler2D` declarations (naga doesn't support standalone uniform vars)
/// - Replace `textureLod(name, uv, lod)` calls with stub function calls
/// - Add stub function definitions for each extracted texture
fn preprocess_glsl(source: &str) -> (String, Vec<TextureInfo>) {
    let mut result = String::with_capacity(source.len());
    let mut textures: Vec<TextureInfo> = Vec::new();

    for line in source.lines() {
        let trimmed = line.trim();

        // Skip precision qualifiers
        if trimmed.starts_with("precision ") {
            result.push('\n');
            continue;
        }

        // Extract and remove uniform sampler2D declarations
        if trimmed.contains("sampler2D") && (trimmed.contains("uniform") || trimmed.contains("layout")) {
            if let Some(name) = extract_sampler_name(trimmed) {
                textures.push(TextureInfo { name });
            }
            // Don't include this line
            result.push('\n');
            continue;
        }

        // Strip default values from non-sampler uniform declarations
        if trimmed.starts_with("uniform ")
            && trimmed.contains(" = ")
            && trimmed.ends_with(';')
        {
            if let Some(eq_pos) = trimmed.rfind(" = ") {
                let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
                result.push_str(&indent);
                result.push_str(&trimmed[..eq_pos]);
                result.push_str(";\n");
                continue;
            }
        }

        // Convert const-qualified globals to regular globals
        // naga's GLSL frontend doesn't support the const qualifier
        if is_const_global(trimmed) {
            let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
            result.push_str(&indent);
            result.push_str(&trimmed["const ".len()..]);
            result.push('\n');
            continue;
        }

        result.push_str(line);
        result.push('\n');
    }

    // Replace textureLod(name, uv, lod) with mm_tex_NAME(uv, lod)
    // and add stub function definitions
    for tex in &textures {
        // Handle textureLod(name, ...) → mm_tex_name(...)
        let old_pattern = format!("textureLod({},", tex.name);
        let new_pattern = format!("mm_tex_{}(", tex.name);
        result = result.replace(&old_pattern, &new_pattern);

        // Also handle textureLod(name , ...) with space before comma
        let old_pattern2 = format!("textureLod({} ,", tex.name);
        result = result.replace(&old_pattern2, &new_pattern);

        // Handle texture(name, ...)
        let old_pattern3 = format!("texture({},", tex.name);
        result = result.replace(&old_pattern3, &new_pattern);

        let old_pattern4 = format!("texture({} ,", tex.name);
        result = result.replace(&old_pattern4, &new_pattern);
    }

    // Insert stub function definitions before the main() function
    if !textures.is_empty() {
        let stub_defs: String = textures
            .iter()
            .map(|tex| {
                format!(
                    "vec4 mm_tex_{}(vec2 uv, float lod) {{ return vec4(0.0, 0.0, 0.0, 1.0); }}\n",
                    tex.name
                )
            })
            .collect();

        // Insert before "void main()"
        if let Some(main_pos) = result.find("void main()") {
            result.insert_str(main_pos, &stub_defs);
        } else if let Some(main_pos) = result.find("void main(") {
            result.insert_str(main_pos, &stub_defs);
        } else {
            // Append at end if no main found
            result.push_str(&stub_defs);
        }
    }

    (result, textures)
}

/// Check if a line is a const-qualified global variable declaration
fn is_const_global(trimmed: &str) -> bool {
    (trimmed.starts_with("const float ")
        || trimmed.starts_with("const vec2 ")
        || trimmed.starts_with("const vec3 ")
        || trimmed.starts_with("const vec4 ")
        || trimmed.starts_with("const int ")
        || trimmed.starts_with("const ivec2 ")
        || trimmed.starts_with("const ivec3 ")
        || trimmed.starts_with("const ivec4 ")
        || trimmed.starts_with("const mat2 "))
        && trimmed.ends_with(';')
}

/// Extract the sampler name from a GLSL sampler declaration line.
/// Handles: "uniform sampler2D name;" and "layout(...) uniform sampler2D name;"
fn extract_sampler_name(line: &str) -> Option<String> {
    // Find "sampler2D" and take the next identifier
    let idx = line.find("sampler2D")?;
    let after = &line[idx + "sampler2D".len()..];
    let after = after.trim_start();
    // The name is the next word before ';' or ' '
    let name: String = after
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// Convert GLSL to WGSL using naga.
fn naga_convert(glsl_source: &str) -> Result<String, String> {
    use naga::back::wgsl;
    use naga::front::glsl::{Frontend, Options};
    use naga::valid::{Capabilities, ValidationFlags, Validator};
    use naga::ShaderStage;

    let mut frontend = Frontend::default();
    let options = Options::from(ShaderStage::Fragment);

    let module = frontend
        .parse(&options, glsl_source)
        .map_err(|errors| {
            let msgs: Vec<String> = errors.errors.iter().map(|e| format!("{}", e)).collect();
            format!(
                "GLSL parse errors:\n{}\n\nPreprocessed GLSL:\n{}",
                msgs.join("\n"),
                glsl_source
                    .lines()
                    .enumerate()
                    .map(|(i, l)| format!("{:4}: {}", i + 1, l))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        })?;

    let mut validator = Validator::new(ValidationFlags::all(), Capabilities::all());
    let info = validator
        .validate(&module)
        .map_err(|e| format!("Validation error: {}", e))?;

    wgsl::write_string(&module, &info, wgsl::WriterFlags::empty())
        .map_err(|e| format!("WGSL write error: {}", e))
}

/// Replace stub texture functions in the WGSL output with actual texture sampling,
/// and add texture/sampler declarations.
fn postprocess_textures(mut wgsl: String, textures: &[TextureInfo]) -> String {
    if textures.is_empty() {
        return wgsl;
    }

    let mut tex_declarations = String::new();

    for (i, tex) in textures.iter().enumerate() {
        let tex_binding = 1 + i * 2;
        let sampler_binding = tex_binding + 1;

        // Add texture and sampler declarations
        tex_declarations.push_str(&format!(
            "@group(0) @binding({}) var {}: texture_2d<f32>;\n\
             @group(0) @binding({}) var {}_sampler: sampler;\n\n",
            tex_binding, tex.name, sampler_binding, tex.name
        ));

        // naga renames our stub function mm_tex_NAME → mm_tex_NAME_ (adds underscore)
        // and creates a wrapper. We need to find and replace the stub function body
        // with actual texture sampling.
        //
        // naga generates the stub as:
        //   fn mm_tex_NAME_(uv: vec2<f32>, lod: f32) -> vec4<f32> {
        //       return vec4<f32>(0f, 0f, 0f, 1f);
        //   }
        //
        // We replace the body with actual texture sampling.

        // Find the stub function definition and replace its body
        let stub_patterns = [
            format!("fn mm_tex_{}_", tex.name),
            format!("fn mm_tex_{}", tex.name),
        ];

        for stub_fn_name in &stub_patterns {
            if let Some(fn_start) = wgsl.find(stub_fn_name.as_str()) {
                // Find the function body (between { and })
                if let Some(body_start) = wgsl[fn_start..].find('{') {
                    let body_start_abs = fn_start + body_start;
                    // Find matching closing brace
                    let mut depth = 0i32;
                    let mut body_end_abs = None;
                    for (offset, ch) in wgsl[body_start_abs..].char_indices() {
                        match ch {
                            '{' => depth += 1,
                            '}' => {
                                depth -= 1;
                                if depth == 0 {
                                    body_end_abs = Some(body_start_abs + offset);
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    if let Some(body_end) = body_end_abs {
                        // Also need to find the parameter names from the function signature
                        let sig = &wgsl[fn_start..body_start_abs];
                        let (uv_param, lod_param) = extract_wgsl_params(sig);

                        let new_body = format!(
                            "{{\n    return textureSampleLevel({}, {}_sampler, {}, {});\n}}",
                            tex.name, tex.name, uv_param, lod_param
                        );
                        wgsl = format!(
                            "{}{}{}",
                            &wgsl[..body_start_abs],
                            new_body,
                            &wgsl[body_end + 1..]
                        );
                        break; // Only replace the first match
                    }
                }
            }
        }
    }

    // Prepend texture declarations
    format!("{}{}", tex_declarations, wgsl)
}

/// Extract parameter names from a WGSL function signature.
/// Returns (uv_param_name, lod_param_name).
fn extract_wgsl_params(sig: &str) -> (String, String) {
    // Signature looks like: fn mm_tex_NAME_(uv_1: vec2<f32>, lod_1: f32) -> vec4<f32>
    let mut uv_name = "uv".to_string();
    let mut lod_name = "0.0".to_string();

    if let Some(paren_start) = sig.find('(') {
        if let Some(paren_end) = sig.find(')') {
            let params = &sig[paren_start + 1..paren_end];
            let parts: Vec<&str> = params.split(',').collect();
            if !parts.is_empty() {
                // First param is UV
                if let Some(colon) = parts[0].find(':') {
                    uv_name = parts[0][..colon].trim().to_string();
                }
            }
            if parts.len() > 1 {
                // Second param is LOD
                if let Some(colon) = parts[1].find(':') {
                    lod_name = parts[1][..colon].trim().to_string();
                }
            }
        }
    }

    (uv_name, lod_name)
}

/// Post-process WGSL output for Bevy compatibility:
/// - Move all resource bindings to @group(2) (Bevy's material bind group)
/// - Rename the fragment entry point from "main" to "fragment"
/// - Rewrite entry point to accept Bevy's VertexOutput
/// - Add Bevy #import directive
fn bevy_postprocess(wgsl: String) -> String {
    // Move all bindings to group(2) — Bevy material bind group
    let wgsl = wgsl
        .replace("@group(0) ", "@group(2) ")
        .replace("@group(1) ", "@group(2) ");

    // Rewrite the @fragment entry point for Bevy
    let wgsl = rewrite_entry_point_for_bevy(wgsl);

    // Prepend Bevy import
    format!("#import bevy_pbr::forward_io::VertexOutput\n\n{}", wgsl)
}

/// Rewrite the @fragment entry point so it accepts Bevy's VertexOutput
/// and provides a `uv` local variable from `in.uv`.
///
/// naga generates:
///   @fragment fn main(@location(0) v_Uv: vec2<f32>) -> FragmentOutput { ... }
///
/// We replace with:
///   @fragment fn fragment(in: VertexOutput) -> @location(0) vec4<f32> { let uv = in.uv; ... }
fn rewrite_entry_point_for_bevy(wgsl: String) -> String {
    let frag_tag = "@fragment";
    let Some(frag_start) = wgsl.find(frag_tag) else {
        return wgsl;
    };

    let after_frag = &wgsl[frag_start + frag_tag.len()..];
    let Some(fn_rel) = after_frag.find("fn ") else {
        return wgsl;
    };
    let fn_abs = frag_start + frag_tag.len() + fn_rel;

    // Find the opening paren
    let Some(paren_rel) = wgsl[fn_abs..].find('(') else {
        return wgsl;
    };
    let paren_abs = fn_abs + paren_rel;

    // Find matching closing paren
    let mut depth = 0i32;
    let mut close_paren_abs = None;
    for (offset, ch) in wgsl[paren_abs..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    close_paren_abs = Some(paren_abs + offset);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close_paren) = close_paren_abs else {
        return wgsl;
    };

    // Extract the location parameters from the naga-generated signature
    let params = &wgsl[paren_abs + 1..close_paren];
    let loc_params = extract_location_params(params);

    // Find the opening brace
    let Some(brace_rel) = wgsl[close_paren..].find('{') else {
        return wgsl;
    };
    let brace_abs = close_paren + brace_rel;

    // Build preamble: map each @location param to the appropriate VertexOutput field
    let mut preamble = String::new();
    let mut replacements: Vec<(String, String)> = Vec::new();

    for p in &loc_params {
        match p.location {
            0 => {
                // UV — always map to in.uv
                preamble.push_str("    let uv = in.uv;\n");
                replacements.push((p.name.clone(), "uv".to_string()));
            }
            1 if p.wgsl_type.contains("vec4") => {
                // World position (used by raymarching) — map to in.world_position
                preamble.push_str("    let world_position_rm = in.world_position.xyz;\n");
                replacements.push((p.name.clone(), "in.world_position".to_string()));
            }
            _ => {
                // Unknown extra input — leave a note but don't crash
                preamble.push_str(&format!(
                    "    // Note: @location({}) param '{}' not automatically mapped\n",
                    p.location, p.name
                ));
            }
        }
    }

    // Build the new function
    let mut result = String::with_capacity(wgsl.len() + 256);
    result.push_str(&wgsl[..frag_start]);
    result.push_str("@fragment\nfn fragment(in: VertexOutput) -> @location(0) vec4<f32> {\n");
    result.push_str(&preamble);

    // Get the rest of the function body (after the opening brace)
    let mut body = wgsl[brace_abs + 1..].to_string();

    // Replace references to each location param with its Bevy equivalent
    for (from, to) in &replacements {
        body = replace_word(&body, from, to);
    }

    // Remove the naga-generated FragmentOutput wrapper
    // naga generates: "return FragmentOutput(_eN);"
    // We want: "return _eN;"
    if let Some(ret_pos) = body.rfind("return FragmentOutput(") {
        let after_ret = &body[ret_pos + "return FragmentOutput(".len()..];
        if let Some(close) = after_ret.find(')') {
            let inner = &after_ret[..close];
            body = format!(
                "{}return {}{}",
                &body[..ret_pos],
                inner,
                &after_ret[close + 1..]
            );
        }
    }

    // Also remove the naga-generated o_Target assignment and use it directly
    // This handles the pattern where naga creates:
    //   o_Target = vec4<f32>(...);
    //   ...
    //   return o_Target;

    result.push_str(&body);

    // Remove the FragmentOutput struct definition (not needed with Bevy's VertexOutput)
    let result = remove_struct_def(&result, "FragmentOutput");

    result
}

/// Typed info about a @location parameter from a WGSL fragment function signature.
#[derive(Debug)]
struct LocationParam {
    /// The @location index
    location: u32,
    /// The naga-generated parameter name
    name: String,
    /// The WGSL type string (e.g. "vec2<f32>", "vec4<f32>")
    wgsl_type: String,
}

/// Extract @location parameters with their types from a WGSL parameter list.
fn extract_location_params(param_list: &str) -> Vec<LocationParam> {
    let mut params = Vec::new();
    for part in param_list.split(',') {
        let part = part.trim();
        if !part.contains("@location") {
            continue;
        }
        // Extract location index: @location(N)
        let location = if let Some(start) = part.find("@location(") {
            let after = &part[start + "@location(".len()..];
            if let Some(end) = after.find(')') {
                after[..end].trim().parse::<u32>().unwrap_or(0)
            } else {
                0
            }
        } else {
            0
        };
        // Extract name: last word before the colon
        if let Some(colon_pos) = part.rfind(':') {
            let before_colon = part[..colon_pos].trim();
            let name = before_colon.split_whitespace().last().unwrap_or("").to_string();
            let wgsl_type = part[colon_pos + 1..].trim().to_string();
            if !name.is_empty() {
                params.push(LocationParam { location, name, wgsl_type });
            }
        }
    }
    params
}

/// Replace whole-word occurrences of `from` with `to` in the given string.
fn replace_word(text: &str, from: &str, to: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.char_indices().peekable();
    while let Some(&(i, _)) = chars.peek() {
        if text[i..].starts_with(from) {
            // Check that it's a word boundary before
            let before_ok = if i == 0 {
                true
            } else {
                let prev = text.as_bytes()[i - 1];
                !prev.is_ascii_alphanumeric() && prev != b'_'
            };

            // Check that it's a word boundary after
            let after_idx = i + from.len();
            let after_ok = if after_idx >= text.len() {
                true
            } else {
                let next = text.as_bytes()[after_idx];
                !next.is_ascii_alphanumeric() && next != b'_'
            };

            if before_ok && after_ok {
                result.push_str(to);
                // Skip the matched characters
                for _ in 0..from.len() {
                    chars.next();
                }
                continue;
            }
        }

        let (_, c) = chars.next().unwrap();
        result.push(c);
    }

    result
}

/// Remove a struct definition from the WGSL source.
fn remove_struct_def(wgsl: &str, name: &str) -> String {
    let pattern = format!("struct {} {{", name);
    let Some(start) = wgsl.find(&pattern) else {
        return wgsl.to_string();
    };

    // Find the matching closing brace
    let mut depth = 0i32;
    let mut end = None;
    for (offset, ch) in wgsl[start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(start + offset + 1);
                    break;
                }
            }
            _ => {}
        }
    }

    if let Some(end) = end {
        // Also skip trailing newlines
        let mut trim_end = end;
        while trim_end < wgsl.len() && wgsl.as_bytes()[trim_end] == b'\n' {
            trim_end += 1;
        }
        format!("{}{}", &wgsl[..start], &wgsl[trim_end..])
    } else {
        wgsl.to_string()
    }
}

