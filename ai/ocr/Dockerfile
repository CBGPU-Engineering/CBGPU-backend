# Build context contains only this Dockerfile and the prebuilt static native worker.
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    poppler-utils imagemagick tesseract-ocr tesseract-ocr-eng tesseract-ocr-kor \
    && rm -rf /var/lib/apt/lists/*
COPY cbgpu-ai-documents /usr/local/bin/cbgpu-ai-documents
ENV OMP_THREAD_LIMIT=2 MAGICK_THREAD_LIMIT=2
ENTRYPOINT ["timeout", "240", "/usr/local/bin/cbgpu-ai-documents"]
