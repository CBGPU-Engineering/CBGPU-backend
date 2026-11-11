"""Offline, container-only PDF/image text extraction with page-level OCR fallback."""
import csv
import json
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image, ImageOps
from pypdf import PdfReader

Image.MAX_IMAGE_PIXELS = 25_000_000
MAX_PAGES = 100
MAX_TEXT = 200_000


def event(value):
    print(json.dumps(value, ensure_ascii=False), flush=True)


def recognize(image, folder):
    image = ImageOps.exif_transpose(image)
    if image.width * image.height > Image.MAX_IMAGE_PIXELS:
        raise ValueError('이미지 해상도가 너무 커요. 2,500만 픽셀 이하로 올려 주세요.')
    image = ImageOps.autocontrast(ImageOps.grayscale(image))
    image.thumbnail((2400, 2400))
    if image.width < 1200 and image.height < 1200:
        image = image.resize((image.width * 2, image.height * 2))
    target = folder / 'page.png'
    image.save(target)
    # Try automatic layout and a single text block. Korean headings can be
    # incorrectly discarded as graphics by the automatic layout detector.
    candidates = []
    for segmentation in (3, 6):
        output = folder / ('recognized-'+str(segmentation))
        subprocess.run(
            ['tesseract', str(target), str(output), '-l', 'kor+eng', '--psm', str(segmentation), 'txt', 'tsv'],
            check=True, timeout=30, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        text = output.with_suffix('.txt').read_text()
        with output.with_suffix('.tsv').open() as source:
            words = [row for row in csv.DictReader(source, delimiter='\t')
                     if row.get('text', '').strip() and float(row['conf']) >= 0]
        length = sum(len(row['text']) for row in words)
        score = sum(len(row['text']) * max(0, float(row['conf'])-35) for row in words)
        confidence = round(sum(len(row['text'])*float(row['conf']) for row in words)/length) if length else 0
        candidates.append((score, text, confidence))
    _, text, confidence = max(candidates, key=lambda value:value[0])
    return text, confidence


def extract(path, suffix):
    pages = []
    ocr_pages = []
    weak_pages = []
    empty_pages = []
    truncated = False
    total = 0
    with tempfile.TemporaryDirectory() as directory:
        folder = Path(directory)
        if suffix == '.pdf':
            pdf = PdfReader(path)
            if pdf.is_encrypted:
                raise ValueError('암호가 걸린 PDF예요. 암호를 해제한 파일을 올려 주세요.')
            count = len(pdf.pages)
            if not 1 <= count <= MAX_PAGES:
                raise ValueError('PDF는 1~100쪽까지 읽을 수 있어요.')
            for index, page in enumerate(pdf.pages, 1):
                event({'progress': index-1, 'pages': count})
                text = page.extract_text() or ''
                meaningful = sum(character.isalnum() for character in text)
                resources = page.get('/Resources', {})
                if hasattr(resources, 'get_object'):
                    resources = resources.get_object()
                objects = resources.get('/XObject', {})
                if hasattr(objects, 'get_object'):
                    objects = objects.get_object()
                has_image = any(obj.get_object().get('/Subtype') == '/Image' for obj in objects.values())
                if meaningful < 40 or (has_image and meaningful < 200):
                    prefix = folder / 'render'
                    subprocess.run(
                        ['pdftoppm', '-f', str(index), '-l', str(index), '-singlefile',
                         '-scale-to', '2200', '-png', path, str(prefix)],
                        check=True, timeout=20, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                    )
                    with Image.open(prefix.with_suffix('.png')) as image:
                        recognized, confidence = recognize(image, folder)
                    if recognized.strip():
                        text = recognized
                        ocr_pages.append(index)
                        if confidence < 65:
                            weak_pages.append(index)
                if not text.strip():
                    empty_pages.append(index)
                if len(text) > 12000 or total + len(text) > MAX_TEXT:
                    truncated = True
                text = text[:min(12000, max(0, MAX_TEXT-total))]
                pages.append((index, text))
                total += len(text)
                event({'progress': index, 'pages': count, 'ocr_pages': len(ocr_pages)})
                if total >= MAX_TEXT:
                    break
        else:
            with Image.open(path) as image:
                count = getattr(image, 'n_frames', 1)
                if count > 20:
                    raise ValueError('여러 장이 담긴 이미지는 20장까지 읽을 수 있어요.')
                for index in range(1, count+1):
                    event({'progress': index-1, 'pages': count})
                    image.seek(index-1)
                    text, confidence = recognize(image.copy(), folder)
                    if len(text) > 12000:
                        truncated = True
                    pages.append((index, text[:12000]))
                    ocr_pages.append(index)
                    if confidence < 65:
                        weak_pages.append(index)
                    if not text.strip():
                        empty_pages.append(index)
                    event({'progress': index, 'pages': count, 'ocr_pages': index})
    chunks = [(index, text[start:start+1000]) for index, text in pages
              for start in range(0, len(text), 1000) if text[start:start+1000].strip()]
    if not chunks:
        raise ValueError('읽을 수 있는 글자를 찾지 못했어요. 글자가 선명하게 보이는 파일을 올려 주세요.')
    return {'chunks': chunks, 'pages': count, 'ocr_page_numbers': ocr_pages,
            'weak_pages': weak_pages, 'empty_pages': empty_pages, 'truncated': truncated}


if __name__ == '__main__':
    try:
        event({'result': extract(sys.argv[1], sys.argv[2])})
    except Exception as error:
        message = str(error) if isinstance(error, ValueError) else '문서를 읽지 못했어요. 선명한 이미지나 올바른 PDF로 다시 올려 주세요.'
        event({'error': message[:300]})
        sys.exit(1)
