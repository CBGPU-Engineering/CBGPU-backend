"""Extract bounded evidence from private chat attachments."""
import csv
import io
import json
import math
import re
import statistics
import subprocess
import sys
import threading
from pathlib import Path

from core import Failure

MAX_BYTES = 8 * 1024 * 1024
PARSER_SLOTS = threading.BoundedSemaphore(2)
TEXT_TYPES = {'.txt', '.md', '.csv', '.json', '.tex', '.py', '.log'}


def csv_summary(text):
    reader = csv.reader(io.StringIO(text), strict=True)
    headers = next(reader, [])
    if not headers or len(headers) > 64:
        raise ValueError('CSV needs a header and at most 64 columns.')
    columns = [[] for _ in headers]
    counts = [0] * len(headers)
    rows = 0
    for row in reader:
        if len(row) != len(headers) or rows >= 100_000:
            raise ValueError('CSV rows must match the header, with at most 100,000 rows.')
        rows += 1
        for index, cell in enumerate(row):
            if not cell.strip():
                continue
            counts[index] += 1
            try:
                value = float(cell)
            except ValueError:
                continue
            if math.isfinite(value) and abs(value) <= 1e100:
                columns[index].append(value)
    lines = [f'Rows: {rows}']
    for index, values in enumerate(columns):
        if not values or len(values) != counts[index]:
            continue
        lines.append(
            f'{headers[index][:100]}: n={len(values)}, missing={rows-counts[index]}, '
            f'mean={statistics.mean(values):.8g}, min={min(values):.8g}, max={max(values):.8g}'
            + (f', sample SD={statistics.stdev(values):.8g}' if len(values) > 1 else '')
        )
    return '\n'.join(lines)


def extract(path, suffix):
    if suffix == '.pdf':
        from pypdf import PdfReader
        pdf = PdfReader(path)
        if pdf.is_encrypted or len(pdf.pages) > 100:
            raise ValueError('Use an unencrypted PDF of up to 100 pages.')
        return [(index, (page.extract_text() or '')[:12000])
                for index, page in enumerate(pdf.pages, 1)]
    text = Path(path).read_text(encoding='utf-8-sig')
    if suffix == '.csv':
        return [(None, csv_summary(text))]
    return [(None, text[start:start+1200]) for start in range(0, min(len(text), 200000), 1200)]


def evidence(store, owner, ids, question):
    if not isinstance(ids, list) or len(ids) > 3 or any(not isinstance(id, str) for id in ids):
        raise Failure('파일은 최대 3개까지 첨부할 수 있어요.')
    words = list(dict.fromkeys(re.findall(r'\w{2,}', question.lower())))[:64]
    results = []
    for id in dict.fromkeys(ids):
        upload = store.get_upload(owner, id)
        suffix = Path(upload['name']).suffix.lower()
        if upload['size'] > MAX_BYTES or suffix not in TEXT_TYPES | {'.pdf'}:
            raise Failure('8MB 이하의 PDF, CSV 또는 텍스트 파일을 첨부해 주세요.')
        python = store.root / 'venv/bin/python' if suffix == '.pdf' else sys.executable
        if not PARSER_SLOTS.acquire(blocking=False):
            raise Failure('다른 첨부 파일을 읽고 있어요. 잠시 후 다시 시도해 주세요.', 429)
        try:
            output = subprocess.check_output(
                [str(python), str(Path(__file__).resolve()), str(store.root / 'uploads' / id), suffix],
                timeout=8, stderr=subprocess.DEVNULL,
            )
            chunks = [(page, text.strip()) for page, text in json.loads(output) if text.strip()]
        except (OSError, ValueError, subprocess.SubprocessError) as error:
            raise Failure('파일을 읽지 못했어요. PDF는 암호 없이 100쪽까지, 텍스트는 UTF-8로 올려 주세요.') from error
        finally:
            PARSER_SLOTS.release()
        if not chunks:
            raise Failure('읽을 수 있는 텍스트가 없어요. 스캔 PDF는 먼저 OCR을 적용해 주세요.')
        ranked = sorted(chunks, key=lambda entry: sum(word in entry[1].lower() for word in words), reverse=True)
        for page, excerpt in ranked[:1 if suffix == '.csv' else 2]:
            results.append({'title': upload['name'], 'page': page, 'excerpt': excerpt[:1000]})
    return results


if __name__ == '__main__':
    import resource
    resource.setrlimit(resource.RLIMIT_AS, (512 * 1024**2, 512 * 1024**2))
    print(json.dumps(extract(sys.argv[1], sys.argv[2]), ensure_ascii=False))
