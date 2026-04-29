use std::path::Path;

use lopdf::Document;

/// Texto extraído de uma página do PDF.
#[derive(Debug, Clone)]
pub struct PageText {
    pub page_num: u32,
    pub text: String,
}

/// Imagem extraída de uma página do PDF.
#[derive(Debug, Clone)]
pub struct PageImage {
    pub page_num: u32,
    /// Bytes raw da imagem (JPEG, PNG ou dados brutos do stream).
    pub bytes: Vec<u8>,
    /// Mime type inferido: "image/jpeg", "image/png" ou "image/raw".
    pub mime: &'static str,
}

#[derive(Debug)]
pub enum ExtractError {
    Io(std::io::Error),
    Pdf(String),
}

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Pdf(msg) => write!(f, "PDF error: {msg}"),
        }
    }
}

impl std::error::Error for ExtractError {}

impl From<std::io::Error> for ExtractError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

// ─── Text ─────────────────────────────────────────────────────────────────────

/// Extrai texto de cada página do PDF.
/// Retorna `Vec<PageText>` ordenado por página.
pub fn extract_text(path: &Path) -> Result<Vec<PageText>, ExtractError> {
    let doc = Document::load(path).map_err(|e| ExtractError::Pdf(e.to_string()))?;
    let pages: Vec<u32> = doc.get_pages().keys().copied().collect();
    let mut results = Vec::with_capacity(pages.len());

    for &page_num in &pages {
        let text = doc
            .extract_text(&[page_num])
            .unwrap_or_default()
            .trim()
            .to_string();
        if !text.is_empty() {
            results.push(PageText { page_num, text });
        }
    }

    results.sort_by_key(|p| p.page_num);
    Ok(results)
}

// ─── Images ───────────────────────────────────────────────────────────────────

/// Extrai imagens embutidas de cada página via XObject.
/// Retorna `Vec<PageImage>` com os bytes brutos e mime type inferido.
pub fn extract_images(path: &Path) -> Result<Vec<PageImage>, ExtractError> {
    let doc = Document::load(path).map_err(|e| ExtractError::Pdf(e.to_string()))?;
    let mut results = Vec::new();

    for (&page_num, &page_id) in doc.get_pages().iter() {
        // Obtém recursos da página
        let resources = match doc.get_page_resources(page_id) {
            Ok((Some(res), _)) => res.clone(),
            _ => continue,
        };

        // Itera sobre XObjects da página
        let xobjects = match resources.get(b"XObject") {
            Ok(obj) => match doc.dereference(obj) {
                Ok((_, lopdf::Object::Dictionary(d))) => d.clone(),
                _ => continue,
            },
            _ => continue,
        };

        for (_, xobj_ref) in xobjects.iter() {
            let stream = match doc.dereference(xobj_ref) {
                Ok((_, lopdf::Object::Stream(s))) => s.clone(),
                _ => continue,
            };

            // Verifica se é imagem (Subtype = Image)
            let subtype = stream
                .dict
                .get(b"Subtype")
                .and_then(|o| o.as_name_str())
                .ok()
                .unwrap_or("");
            if subtype != "Image" {
                continue;
            }

            let bytes = stream.content.clone();
            if bytes.is_empty() {
                continue;
            }

            // Infere mime pelo filtro do stream
            let mime = stream
                .dict
                .get(b"Filter")
                .and_then(|o| o.as_name_str())
                .ok()
                .map(|f| match f {
                    "DCTDecode" => "image/jpeg",
                    "FlateDecode" | "PNG" => "image/png",
                    _ => "image/raw",
                })
                .unwrap_or("image/raw");

            results.push(PageImage { page_num, bytes, mime });
        }
    }

    results.sort_by_key(|i| i.page_num);
    Ok(results)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_text_nonexistent_returns_error() {
        let result = extract_text(Path::new("/nonexistent/file.pdf"));
        assert!(result.is_err());
    }

    #[test]
    fn extract_images_nonexistent_returns_error() {
        let result = extract_images(Path::new("/nonexistent/file.pdf"));
        assert!(result.is_err());
    }
}
