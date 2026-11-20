//! A small PDF written by hand, for tests here and in the clients.

/// A PDF of `pages` pages of 200 × 100 points, each with a black square near its bottom left.
pub fn sample_pdf(pages: usize) -> Vec<u8> {
    let content = "0 0 0 rg 10 10 50 50 re f";
    let kids: Vec<_> = (0..pages).map(|page| format!("{} 0 R", 4 + page)).collect();
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        format!(
            "<< /Type /Pages /Kids [{}] /Count {pages} >>",
            kids.join(" ")
        ),
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ),
    ];
    objects.extend((0..pages).map(|_| {
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 3 0 R >>".to_owned()
    }));
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = pdf.len();
    let size = objects.len() + 1;
    pdf.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    pdf
}
