"""Dev-only fixture generator for PDF test fixtures (see SPEC.md §5.1
fixtures/corpus, §7 M2). Not a runtime dependency — mirrors
tools/reference_embeddings.py's role as dev tooling. Run once locally;
outputs are checked into the repo.

Usage: python fixtures/corpus/generate_pdfs.py
Requires: pip install reportlab
"""

from pathlib import Path

from reportlab.lib.pagesizes import LETTER
from reportlab.lib import pdfencrypt
from reportlab.pdfgen import canvas

ROOT = Path(__file__).parent


def make_multi_page_pdf(path: Path, pages: list[str]) -> None:
    c = canvas.Canvas(str(path), pagesize=LETTER)
    for text in pages:
        c.setFont("Helvetica", 12)
        y = 750
        for line in text.splitlines():
            c.drawString(72, y, line)
            y -= 16
        c.showPage()
    c.save()


def make_encrypted_pdf(path: Path, text: str, user_password: str) -> None:
    enc = pdfencrypt.StandardEncryption(userPassword=user_password, ownerPassword="owner-secret")
    c = canvas.Canvas(str(path), pagesize=LETTER, encrypt=enc)
    c.setFont("Helvetica", 12)
    c.drawString(72, 750, text)
    c.showPage()
    c.save()


def main() -> None:
    pdf_dir = ROOT / "pdf"
    edge_dir = ROOT / "edge"

    make_multi_page_pdf(
        pdf_dir / "report.pdf",
        [
            "Quarterly Report - Page One\nRevenue grew steadily this quarter.",
            "Quarterly Report - Page Two\nExpenses were kept under control.",
            "Quarterly Report - Page Three\nOutlook remains positive for next year.",
        ],
    )

    make_multi_page_pdf(
        pdf_dir / "factura_electricista.pdf",
        [
            "Factura de electricista\nServicio: reparacion del panel electrico\nTotal: 150 euros",
        ],
    )

    make_encrypted_pdf(
        edge_dir / "password_protected.pdf",
        "This content requires a password to read.",
        user_password="hunter2",
    )

    # A truncated (corrupt) PDF: a valid file cut off mid-stream, so PDFium
    # must fail to parse it cleanly rather than crash.
    good = pdf_dir / "report.pdf"
    truncated = edge_dir / "truncated.pdf"
    data = good.read_bytes()
    truncated.write_bytes(data[: len(data) // 3])

    print("wrote:", pdf_dir / "report.pdf")
    print("wrote:", pdf_dir / "factura_electricista.pdf")
    print("wrote:", edge_dir / "password_protected.pdf")
    print("wrote:", truncated)


if __name__ == "__main__":
    main()
