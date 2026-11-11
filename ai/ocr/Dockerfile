FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    python3 python3-pil python3-pypdf poppler-utils \
    tesseract-ocr tesseract-ocr-eng tesseract-ocr-kor \
    && rm -rf /var/lib/apt/lists/*
ENV OMP_THREAD_LIMIT=2 PYTHONDONTWRITEBYTECODE=1
ENTRYPOINT ["timeout", "240", "python3", "/extract.py"]
