"""Convert Apple Pages documents (.pages, Pages 5 / 2013 and later) to Word .docx.

Pages stores a document as a ZIP of IWA files: Snappy-compressed streams of
protobuf messages. Apple publishes no schema, so messages are decoded
schema-less here; the field numbers used below were taken from real documents
and from the reverse-engineering work in libetonyek and keynote-parser.

Only the Python standard library is used.
"""
from __future__ import annotations

import argparse
import bisect
import datetime
import decimal
import io
import os
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import zipfile
from dataclasses import dataclass, field
from xml.sax.saxutils import escape

# User-facing texts. Kept in one place so the tool can be translated without touching the logic.
MESSAGES = {
    'title': 'Pages öffnen',
    'old_format': 'Diese Datei stammt aus Pages 2009 oder älter (iWork ’09). '
                  'Dieses alte Format wird nicht unterstützt.',
    'not_pages': 'Das ist keine lesbare Pages-Datei:\n{path}',
    'failed': 'Die Datei konnte nicht umgewandelt werden:\n{path}\n\n{error}',
    'saved': 'Gespeichert: {path}',
    'reused': 'Bereits umgewandelt, öffne vorhandene Datei: {path}',
    'skipped': 'Hinweis: {count} Objekt(e) wurden nicht übernommen ({kinds}).',
    'no_body': 'Hinweis: Layout-Dokument ohne Fließtext, nur Textfelder wurden übernommen.',
    'usage_output': '--output geht nur mit genau einer Eingabedatei.',
}

WORDCRAFT_CANDIDATES = [
    r'C:\Program Files\ArtCraft\WordCraft\wordcraft.exe',
    os.path.expandvars(r'%LOCALAPPDATA%\Programs\ArtCraft\WordCraft\wordcraft.exe'),
]

# IWA message type ids (from the TSWP/TST/TP registries).
T_STORAGE = 2001
T_DRAWABLE_ATTACHMENT = 2003
T_HYPERLINK = 2032
T_TABLE_INFO = 6000
T_DATA_LIST = 6005
T_PACKAGE_METADATA = 11006

PT = 20            # twips per point
EMU_PER_PT = 12700
APPLE_EPOCH = datetime.datetime(2001, 1, 1, tzinfo=datetime.timezone.utc)


class PagesError(Exception):
    """A document that cannot be converted, with a message for the user."""


# --------------------------------------------------------------------------- decoding

def _varint(buf: bytes, pos: int) -> tuple[int, int]:
    result = shift = 0
    while True:
        b = buf[pos]
        pos += 1
        result |= (b & 0x7F) << shift
        if b < 0x80:
            return result, pos
        shift += 7


def snappy_decompress(buf: bytes) -> bytes:
    """Decompress one raw Snappy block (IWA uses Snappy without framing or CRCs)."""
    expected, pos = _varint(buf, 0)
    out = bytearray()
    end = len(buf)
    while pos < end:
        tag = buf[pos]
        pos += 1
        kind = tag & 3
        if kind == 0:
            n = tag >> 2
            if n >= 60:
                extra = n - 59
                n = int.from_bytes(buf[pos:pos + extra], 'little')
                pos += extra
            n += 1
            out += buf[pos:pos + n]
            pos += n
            continue
        if kind == 1:
            n = 4 + ((tag >> 2) & 7)
            offset = ((tag >> 5) << 8) | buf[pos]
            pos += 1
        elif kind == 2:
            n = (tag >> 2) + 1
            offset = int.from_bytes(buf[pos:pos + 2], 'little')
            pos += 2
        else:
            n = (tag >> 2) + 1
            offset = int.from_bytes(buf[pos:pos + 4], 'little')
            pos += 4
        if offset == 0 or offset > len(out):
            raise ValueError('corrupt snappy block')
        start = len(out) - offset
        if offset >= n:
            out += out[start:start + n]
        else:
            # Overlapping copy is Snappy's run-length encoding: the source grows while copying.
            for i in range(n):
                out.append(out[start + i])
    if len(out) != expected:
        raise ValueError('snappy length mismatch')
    return bytes(out)


def iwa_decompress(data: bytes) -> bytes:
    """Join the Snappy chunks of an .iwa file into the raw archive stream."""
    out = bytearray()
    pos = 0
    while pos < len(data):
        if data[pos] != 0:
            raise ValueError('unexpected IWA chunk type')
        n = int.from_bytes(data[pos + 1:pos + 4], 'little')
        out += snappy_decompress(data[pos + 4:pos + 4 + n])
        pos += 4 + n
    return bytes(out)


class Msg:
    """A protobuf message decoded without a schema.

    Fields map to lists of ``(wire_type, raw_value)``. The typed accessors
    interpret raw values on demand, because the wire format alone cannot tell a
    string from a nested message or a packed array.
    """

    __slots__ = ('fields',)

    def __init__(self, buf: bytes = b''):
        fields: dict[int, list] = {}
        pos, end = 0, len(buf)
        while pos < end:
            key, pos = _varint(buf, pos)
            wt = key & 7
            if wt == 0:
                value, pos = _varint(buf, pos)
            elif wt == 2:
                n, pos = _varint(buf, pos)
                value = buf[pos:pos + n]
                pos += n
            elif wt == 5:
                value = buf[pos:pos + 4]
                pos += 4
            elif wt == 1:
                value = buf[pos:pos + 8]
                pos += 8
            else:
                raise ValueError(f'unsupported wire type {wt}')
            if pos > end:
                raise ValueError('truncated message')
            fields.setdefault(key >> 3, []).append((wt, value))
        self.fields = fields

    @classmethod
    def merged(cls, fields: dict[int, list]) -> Msg:
        m = cls()
        m.fields = fields
        return m

    def has(self, n: int) -> bool:
        return n in self.fields

    def int_(self, n: int, default=None):
        # Last occurrence wins, as protobuf specifies for singular fields.
        for wt, v in reversed(self.fields.get(n, ())):
            if wt == 0:
                return v
            if wt == 5:
                return struct.unpack('<I', v)[0]
            if wt == 1:
                return struct.unpack('<Q', v)[0]
        return default

    def bool_(self, n: int, default=None):
        v = self.int_(n)
        return default if v is None else bool(v)

    def float_(self, n: int, default=None):
        for wt, v in reversed(self.fields.get(n, ())):
            if wt == 5:
                return struct.unpack('<f', v)[0]
            if wt == 1:
                return struct.unpack('<d', v)[0]
            if wt == 0:
                return float(v)
        return default

    def str_(self, n: int, default=None):
        for wt, v in reversed(self.fields.get(n, ())):
            if wt == 2:
                return v.decode('utf-8', 'replace')
        return default

    def strs(self, n: int) -> list[str]:
        return [v.decode('utf-8', 'replace') for wt, v in self.fields.get(n, ()) if wt == 2]

    def msg(self, n: int) -> Msg | None:
        parts = [v for wt, v in self.fields.get(n, ()) if wt == 2]
        if not parts:
            return None
        try:
            # Repeated occurrences of a singular message field are merged by concatenation.
            return Msg(b''.join(parts))
        except (ValueError, IndexError):
            return None

    def msgs(self, n: int) -> list[Msg]:
        out = []
        for wt, v in self.fields.get(n, ()):
            if wt == 2:
                try:
                    out.append(Msg(v))
                except (ValueError, IndexError):
                    pass
        return out

    def ints(self, n: int) -> list[int]:
        out = []
        for wt, v in self.fields.get(n, ()):
            if wt == 0:
                out.append(v)
            elif wt == 2:
                pos = 0
                while pos < len(v):
                    x, pos = _varint(v, pos)
                    out.append(x)
        return out

    def floats(self, n: int) -> list[float]:
        out = []
        for wt, v in self.fields.get(n, ()):
            if wt == 5:
                out.append(struct.unpack('<f', v)[0])
            elif wt == 2 and len(v) % 4 == 0:
                out.extend(struct.unpack(f'<{len(v) // 4}f', v))
        return out

    def ref(self, n: int) -> int | None:
        """Follow a ``TSP.Reference`` field (a message whose field 1 is an object id)."""
        m = self.msg(n)
        return m.int_(1) if m else None

    def refs(self, n: int) -> list[int]:
        return [r for r in (m.int_(1) for m in self.msgs(n)) if r is not None]


EMPTY = Msg()


# --------------------------------------------------------------------------- package

class _Container:
    """Uniform read access to a zipped .pages file or a .pages bundle directory."""

    def __init__(self, path: str):
        self._zip = None
        self._dir = None
        if os.path.isdir(path):
            self._dir = path
            names = []
            for root, _, files in os.walk(path):
                for f in files:
                    names.append(os.path.relpath(os.path.join(root, f), path).replace(os.sep, '/'))
        else:
            try:
                self._zip = zipfile.ZipFile(path)
            except (zipfile.BadZipFile, OSError) as e:
                raise PagesError(MESSAGES['not_pages'].format(path=path)) from e
            names = [n for n in self._zip.namelist() if not n.endswith('/')]
        # A zipped bundle may carry the bundle folder name as prefix ("Doc.pages/Index/...").
        prefix = ''
        for n in names:
            m = re.match(r'(.*?)(Index/Document\.iwa|Index\.zip|index\.xml(\.gz)?)$', n)
            if m and '/' not in m.group(1).rstrip('/'):
                prefix = m.group(1)
                break
        self.prefix = prefix
        self.names = [n[len(prefix):] for n in names if n.startswith(prefix)]

    def read(self, name: str) -> bytes:
        full = self.prefix + name
        if self._zip is not None:
            return self._zip.read(full)
        with open(os.path.join(self._dir, full), 'rb') as f:
            return f.read()


class PagesPackage:
    """Decoded objects and embedded files of one .pages document."""

    def __init__(self, path: str):
        self.container = _Container(path)
        names = set(self.container.names)
        iwa_source = self.container
        iwa_names = [n for n in names if n.startswith('Index/') and n.endswith('.iwa')]
        if not iwa_names and 'Index.zip' in names:
            # Pages 5.0-5.2 bundles keep the IWA files in a nested Index.zip.
            inner = zipfile.ZipFile(io.BytesIO(self.container.read('Index.zip')))
            iwa_source = _ZipReader(inner)
            iwa_names = [n for n in inner.namelist() if n.endswith('.iwa')]
        if not iwa_names:
            if any(n.lower() in ('index.xml', 'index.xml.gz') for n in names):
                raise PagesError(MESSAGES['old_format'])
            raise PagesError(MESSAGES['not_pages'].format(path=path))

        self._objects: dict[int, tuple[int, bytes]] = {}
        self._cache: dict[int, Msg] = {}
        # Document.iwa first so that duplicate ids resolve to the main document's copy.
        for name in sorted(iwa_names, key=lambda n: (not n.endswith('Document.iwa'), n)):
            self._load_iwa(iwa_source.read(name))

        self.data_files = {n[len('Data/'):]: n for n in names if n.startswith('Data/')}
        self.data_map = self._build_data_map()

    def _load_iwa(self, data: bytes) -> None:
        buf = iwa_decompress(data)
        pos = 0
        while pos < len(buf):
            n, pos = _varint(buf, pos)
            info = Msg(buf[pos:pos + n])
            pos += n
            ident = info.int_(1)
            for i, mi in enumerate(info.msgs(2)):
                length = mi.int_(3, 0)
                # Later message infos of one archive are compatibility copies of the first.
                if i == 0 and ident is not None and ident not in self._objects:
                    self._objects[ident] = (mi.int_(1, 0), buf[pos:pos + length])
                pos += length

    def _build_data_map(self) -> dict[int, str]:
        mapping: dict[int, str] = {}
        for oid, (typ, _) in self._objects.items():
            if typ != T_PACKAGE_METADATA:
                continue
            for entry in self.get(oid).msgs(4):
                ident = entry.int_(1)
                if ident is None:
                    continue
                for fn in sorted(entry.fields):
                    s = entry.str_(fn)
                    if s and s in self.data_files:
                        mapping[ident] = self.data_files[s]
                        break
        return mapping

    def type(self, oid: int | None) -> int | None:
        if oid is None or oid not in self._objects:
            return None
        return self._objects[oid][0]

    def get(self, oid: int | None) -> Msg:
        if oid is None or oid not in self._objects:
            return EMPTY
        m = self._cache.get(oid)
        if m is None:
            try:
                m = Msg(self._objects[oid][1])
            except (ValueError, IndexError):
                m = EMPTY
            self._cache[oid] = m
        return m

    def ids_of_type(self, typ: int) -> list[int]:
        return sorted(oid for oid, (t, _) in self._objects.items() if t == typ)

    def read_data(self, name: str) -> bytes:
        return self.container.read(name)


class _ZipReader:
    def __init__(self, z: zipfile.ZipFile):
        self._z = z

    def read(self, name: str) -> bytes:
        return self._z.read(name)


# --------------------------------------------------------------------------- styles

class Styles:
    """Resolves Pages style inheritance (TSS.StyleArchive parent chains)."""

    def __init__(self, pkg: PagesPackage):
        self.pkg = pkg
        self._props: dict[tuple, Msg] = {}

    def chain(self, style_id: int | None) -> list[Msg]:
        """Return the style and its ancestors, root first."""
        out, seen = [], set()
        while style_id is not None and style_id not in seen and len(out) < 32:
            seen.add(style_id)
            style = self.pkg.get(style_id)
            if style is EMPTY:
                break
            out.append(style)
            sup = style.msg(1)
            style_id = sup.ref(3) if sup else None
        out.reverse()
        return out

    def props(self, style_id: int | None, prop_field: int) -> Msg:
        """Merged property message (e.g. field 11 = character, 12 = paragraph properties)."""
        key = (style_id, prop_field)
        if key not in self._props:
            fields: dict[int, list] = {}
            for style in self.chain(style_id):
                p = style.msg(prop_field)
                if p:
                    fields.update(p.fields)
            self._props[key] = Msg.merged(fields)
        return self._props[key]

    def list_style(self, style_id: int | None) -> Msg:
        """List styles keep their properties at top level, so merge everything but the header."""
        key = (style_id, 'list')
        if key not in self._props:
            fields: dict[int, list] = {}
            for style in self.chain(style_id):
                fields.update({k: v for k, v in style.fields.items() if k not in (1, 10)})
            self._props[key] = Msg.merged(fields)
        return self._props[key]

    def identifier(self, style_id: int | None) -> str:
        """The closest non-empty built-in identifier, e.g. ``text-11-paragraphstyle-Heading 1``."""
        for style in reversed(self.chain(style_id)):
            sup = style.msg(1)
            ident = sup.str_(2) if sup else None
            if ident:
                return ident
        return ''


def merge_props(*props: Msg) -> Msg:
    fields: dict[int, list] = {}
    for p in props:
        if p:
            fields.update(p.fields)
    return Msg.merged(fields)


# --------------------------------------------------------------------------- fonts

def _installed_font_families() -> set[str] | None:
    try:
        import winreg
    except ImportError:
        return None
    style_words = {'bold', 'italic', 'oblique', 'light', 'semilight', 'semibold', 'black', 'regular',
                   'medium', 'thin', 'heavy', 'condensed', 'narrow', 'extra', 'extralight', 'demi',
                   'ultra', 'book', '&'}
    families: set[str] = set()
    for hive in (winreg.HKEY_LOCAL_MACHINE, winreg.HKEY_CURRENT_USER):
        try:
            key = winreg.OpenKey(hive, r'SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts')
        except OSError:
            continue
        i = 0
        while True:
            try:
                name = winreg.EnumValue(key, i)[0]
            except OSError:
                break
            i += 1
            name = re.sub(r'\s*\(.*?\)\s*$', '', name).strip()
            for part in name.split(' & '):
                words = part.lower().split()
                families.add(' '.join(words))
                while words and words[-1] in style_words:
                    words.pop()
                    families.add(' '.join(words))
    return families


class FontMapper:
    """Maps Pages' PostScript font names to Windows font families."""

    KNOWN = {
        'helveticaneue': 'Helvetica Neue', 'helvetica': 'Helvetica', 'arial': 'Arial',
        'timesnewroman': 'Times New Roman', 'times': 'Times', 'couriernew': 'Courier New',
        'courier': 'Courier', 'trebuchetms': 'Trebuchet MS', 'comicsansms': 'Comic Sans MS',
        'avenirnext': 'Avenir Next', 'avenirnextcondensed': 'Avenir Next Condensed',
        'avenir': 'Avenir', 'gillsans': 'Gill Sans', 'hoeflertext': 'Hoefler Text',
        'americantypewriter': 'American Typewriter', 'markerfelt': 'Marker Felt',
        'sfpro': 'SF Pro', 'sfprotext': 'SF Pro Text', 'sfprodisplay': 'SF Pro Display',
        'sfmono': 'SF Mono', 'lucidagrande': 'Lucida Grande', 'palatino': 'Palatino',
        'baskerville': 'Baskerville', 'georgia': 'Georgia', 'verdana': 'Verdana',
        'chalkboardse': 'Chalkboard SE', 'bradleyhand': 'Bradley Hand',
        'snellroundhand': 'Snell Roundhand', 'applechancery': 'Apple Chancery',
    }
    # Used only when the original family is not installed.
    SUBSTITUTES = {
        'helvetica neue': 'Arial', 'helvetica': 'Arial', 'sf pro': 'Segoe UI',
        'sf pro text': 'Segoe UI', 'sf pro display': 'Segoe UI', '.sf ns': 'Segoe UI',
        'system font': 'Segoe UI', 'lucida grande': 'Segoe UI', 'avenir': 'Segoe UI',
        'avenir next': 'Segoe UI', 'avenir next condensed': 'Segoe UI', 'seravek': 'Segoe UI',
        'geneva': 'Verdana', 'futura': 'Century Gothic', 'gill sans': 'Gill Sans MT',
        'optima': 'Candara', 'times': 'Times New Roman', 'hoefler text': 'Georgia',
        'cochin': 'Georgia', 'baskerville': 'Georgia', 'big caslon': 'Georgia',
        'charter': 'Georgia', 'iowan old style': 'Georgia', 'didot': 'Bodoni MT',
        'palatino': 'Palatino Linotype', 'american typewriter': 'Courier New',
        'courier': 'Courier New', 'menlo': 'Consolas', 'monaco': 'Consolas', 'sf mono': 'Consolas',
        'marker felt': 'Comic Sans MS', 'chalkboard': 'Comic Sans MS',
        'chalkboard se': 'Comic Sans MS', 'noteworthy': 'Comic Sans MS',
        'chalkduster': 'Comic Sans MS', 'bradley hand': 'Segoe Print',
        'snell roundhand': 'Segoe Script', 'zapfino': 'Segoe Script',
        'apple chancery': 'Segoe Script',
    }

    # Line pitch of a single-spaced line in Pages, in em. Measured from a Pages-rendered preview
    # (12 pt at 1.5 lines = 20.97 pt): Pages uses ascent + descent without the font's line gap.
    # Documents that paginate with blank lines only keep their page breaks if this matches.
    PAGES_LINE_EM = {'helvetica neue': 1.165}

    def __init__(self):
        self.installed = _installed_font_families()
        self._cache: dict[str, tuple[str, bool, bool]] = {}
        self._original: dict[str, str] = {}

    def pages_line_em(self, ps_name: str | None) -> float | None:
        """Pages' single line pitch for this font in em, if it has been measured."""
        if not ps_name:
            return None
        self.resolve(ps_name)
        return self.PAGES_LINE_EM.get(self._original[ps_name].lower())

    def _is_installed(self, family: str) -> bool:
        return self.installed is None or family.lower() in self.installed

    def resolve(self, ps_name: str) -> tuple[str, bool, bool]:
        """Return ``(family, implied_bold, implied_italic)`` for a PostScript font name."""
        if ps_name in self._cache:
            return self._cache[ps_name]
        if ' ' in ps_name:
            base, style = ps_name, ''
        else:
            base, _, style = ps_name.partition('-')
        key = re.sub(r'(psmt|mt|ps)$', '', base.lower())
        family = self.KNOWN.get(key) or re.sub(r'(?<=[a-z])(?=[A-Z])', ' ', re.sub(r'(PSMT|MT|PS)$', '', base))
        self._original[ps_name] = family
        bold = bool(re.search(r'Bold|Heavy|Black|Semibold|SemiBold|Demi', style))
        italic = bool(re.search(r'Italic|Oblique', style))
        weight = re.sub(r'Bold|Italic|Oblique|Regular|Roman|Book', '', style)
        if weight and self.installed is not None and f'{family} {weight}'.lower() in self.installed:
            family = f'{family} {weight}'
        elif not self._is_installed(family):
            sub = self.SUBSTITUTES.get(family.lower(), 'Arial')
            if self._is_installed(sub):
                family = sub
        self._cache[ps_name] = (family, bold, italic)
        return self._cache[ps_name]


# --------------------------------------------------------------------------- docx output

def _attr(s: str) -> str:
    return escape(s, {'"': '&quot;'})


_INVALID_XML = re.compile('[\x00-\x08\x0b\x0c\x0e-\x1f\ud800-\udfff\ufffe\uffff]')


def _text(s: str) -> str:
    return escape(_INVALID_XML.sub('', s))


def _color_hex(c: Msg | None) -> str | None:
    if not c or not c.has(3):
        return None
    rgb = [max(0, min(255, round((c.float_(i, 0.0) or 0.0) * 255))) for i in (3, 4, 5)]
    return '%02X%02X%02X' % tuple(rgb)


def _image_info(data: bytes) -> tuple[str, int, int] | None:
    """Return ``(extension, width_px, height_px)`` for formats Word can embed."""
    if data[:8] == b'\x89PNG\r\n\x1a\n' and len(data) >= 24:
        w, h = struct.unpack('>II', data[16:24])
        return 'png', w, h
    if data[:6] in (b'GIF87a', b'GIF89a'):
        w, h = struct.unpack('<HH', data[6:10])
        return 'gif', w, h
    if data[:2] == b'BM' and len(data) >= 26:
        w, h = struct.unpack('<ii', data[18:26])
        return 'bmp', w, abs(h)
    if data[:2] == b'\xff\xd8':
        pos = 2
        while pos + 9 < len(data):
            if data[pos] != 0xFF:
                pos += 1
                continue
            marker = data[pos + 1]
            if marker in (0xC0, 0xC1, 0xC2, 0xC3, 0xC5, 0xC6, 0xC7, 0xC9, 0xCA, 0xCB, 0xCD, 0xCE, 0xCF):
                h, w = struct.unpack('>HH', data[pos + 5:pos + 9])
                return 'jpeg', w, h
            if marker in (0xD8, 0x01) or 0xD0 <= marker <= 0xD7:
                pos += 2
                continue
            pos += 2 + struct.unpack('>H', data[pos + 2:pos + 4])[0]
        return 'jpeg', 0, 0
    if data[:4] in (b'II*\x00', b'MM\x00*'):
        return 'tiff', 0, 0
    return None


def _convert_image_with_magick(data: bytes) -> bytes | None:
    """Turn HEIC/PDF/other images into PNG when ImageMagick happens to be installed."""
    exe = shutil.which('magick')
    if not exe:
        return None
    with tempfile.TemporaryDirectory() as tmp:
        src, dst = os.path.join(tmp, 'in'), os.path.join(tmp, 'out.png')
        with open(src, 'wb') as f:
            f.write(data)
        try:
            subprocess.run([exe, src + '[0]', dst], check=True, capture_output=True, timeout=60,
                           creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
            with open(dst, 'rb') as f:
                return f.read()
        except (subprocess.SubprocessError, OSError):
            return None


NUMBER_FORMATS = {  # TSWP.ListStyleArchive.NumberType -> (numFmt, prefix, suffix)
    0: ('decimal', '', '.'), 1: ('decimal', '(', ')'), 2: ('decimal', '', ')'),
    3: ('upperRoman', '', '.'), 4: ('upperRoman', '(', ')'), 5: ('upperRoman', '', ')'),
    6: ('lowerRoman', '', '.'), 7: ('lowerRoman', '(', ')'), 8: ('lowerRoman', '', ')'),
    9: ('upperLetter', '', '.'), 10: ('upperLetter', '(', ')'), 11: ('upperLetter', '', ')'),
    12: ('lowerLetter', '', '.'), 13: ('lowerLetter', '(', ')'), 14: ('lowerLetter', '', ')'),
}

HEADING_STYLES = {  # Pages built-in identifier suffix -> (docx style id, name, outline level)
    'Title': ('Title', 'Title', None),
    'Subtitle': ('Subtitle', 'Subtitle', None),
    'Heading 1': ('Heading1', 'heading 1', 0),
    'Heading 2': ('Heading2', 'heading 2', 1),
    'Heading 3': ('Heading3', 'heading 3', 2),
    'Heading 4': ('Heading4', 'heading 4', 3),
}


class Numbering:
    """Collects list definitions for word/numbering.xml."""

    def __init__(self, styles: Styles):
        self.styles = styles
        self.abstract: dict[int, int] = {}
        self.nums: list[tuple[int, int, int | None]] = []   # (numId, abstractId, restart value)
        self.current: dict[int, int] = {}

    def is_list(self, list_style_id: int | None, level: int) -> bool:
        if list_style_id is None:
            return False
        types = self.styles.list_style(list_style_id).ints(11)
        if not types:
            return False
        return types[min(level, len(types) - 1)] != 0

    def num_id(self, list_style_id: int, restart: bool, start: int) -> int:
        if list_style_id not in self.abstract:
            self.abstract[list_style_id] = len(self.abstract)
        if restart or list_style_id not in self.current:
            num = len(self.nums) + 1
            # Word continues numbering across w:num instances of one abstractNum unless
            # each new instance overrides its start value.
            override = start if list_style_id in self.current else None
            self.nums.append((num, self.abstract[list_style_id], override))
            self.current[list_style_id] = num
        return self.current[list_style_id]

    def xml(self) -> str:
        parts = []
        for ls_id, abs_id in self.abstract.items():
            ls = self.styles.list_style(ls_id)
            label_types, scales, indents = ls.ints(11), ls.floats(12), ls.floats(13)
            num_types, bullets, tiered = ls.ints(15), ls.strs(16), ls.ints(25)

            def pick(values, lvl, default):
                if not values:
                    return default
                return values[min(lvl, len(values) - 1)]

            levels = []
            for lvl in range(9):
                lt = pick(label_types, lvl, 0)
                # Pages stores the text indent in multiples of the 11 pt default body size.
                offset = max(pick(scales, lvl, 1.6364) * 11.0, 9.0)
                label = pick(indents, lvl, lvl * 18.0)
                if lt == 3:
                    fmt, pre, suf = NUMBER_FORMATS.get(pick(num_types, lvl, 0), NUMBER_FORMATS[0])
                    if pick(tiered, lvl, 0):
                        text = pre + '.'.join(f'%{k + 1}' for k in range(lvl + 1)) + suf
                    else:
                        text = f'{pre}%{lvl + 1}{suf}'
                elif lt in (1, 2):
                    fmt, text = 'bullet', (pick(bullets, lvl, '•') or '•')
                else:
                    fmt, text = 'none', ''
                levels.append(
                    f'<w:lvl w:ilvl="{lvl}"><w:start w:val="1"/><w:numFmt w:val="{fmt}"/>'
                    f'<w:lvlText w:val="{_attr(text)}"/><w:lvlJc w:val="left"/>'
                    f'<w:pPr><w:ind w:left="{round((label + offset) * PT)}" '
                    f'w:hanging="{round(offset * PT)}"/></w:pPr></w:lvl>')
            parts.append(f'<w:abstractNum w:abstractNumId="{abs_id}">'
                         f'<w:multiLevelType w:val="hybridMultilevel"/>{"".join(levels)}</w:abstractNum>')
        for num, abs_id, override in self.nums:
            extra = ''
            if override is not None:
                extra = ''.join(f'<w:lvlOverride w:ilvl="{lvl}"><w:startOverride w:val="'
                                f'{override if lvl == 0 else 1}"/></w:lvlOverride>' for lvl in range(9))
            parts.append(f'<w:num w:numId="{num}"><w:abstractNumId w:val="{abs_id}"/>{extra}</w:num>')
        return (f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
                f'<w:numbering {NS}>{"".join(parts)}</w:numbering>')


NS = ('xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" '
      'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" '
      'xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" '
      'xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" '
      'xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"')
REL_BASE = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships'


@dataclass
class Context:
    """Where text is being rendered; headers and footers cannot use document relationships."""
    part: str = 'body'
    max_width_pt: float = 481.9

    @property
    def rels_allowed(self) -> bool:
        return self.part != 'header'


@dataclass
class Report:
    skipped: dict[str, int] = field(default_factory=dict)
    notes: list[str] = field(default_factory=list)

    def skip(self, kind: str) -> None:
        self.skipped[kind] = self.skipped.get(kind, 0) + 1


class Converter:
    """Turns a decoded Pages package into the parts of a .docx file."""

    def __init__(self, pkg: PagesPackage):
        self.pkg = pkg
        self.styles = Styles(pkg)
        self.fonts = FontMapper()
        self.numbering = Numbering(self.styles)
        self.report = Report()
        self.rels: list[tuple[str, str, str, bool]] = []
        self.media: list[tuple[str, bytes]] = []
        self._link_rels: dict[str, str] = {}
        self._drawing_id = 0
        self.used_heading_styles: set[str] = set()
        self.decimal_comma = True
        self._rendering: set[int] = set()
        self.line_extras: dict[float, int] = {}   # extra leading (pt) -> paragraph count

    # ----------------------------------------------------------- relationships

    def _add_rel(self, rel_type: str, target: str, external: bool = False) -> str:
        rid = f'rId{len(self.rels) + 10}'
        self.rels.append((rid, f'{REL_BASE}/{rel_type}', target, external))
        return rid

    def _hyperlink_rel(self, url: str) -> str:
        if url not in self._link_rels:
            self._link_rels[url] = self._add_rel('hyperlink', url, external=True)
        return self._link_rels[url]

    # ----------------------------------------------------------- formatting

    def rpr(self, cp: Msg) -> str:
        parts = []
        bold, italic = cp.bool_(1, False), cp.bool_(2, False)
        font = cp.str_(5)
        if font:
            family, fb, fi = self.fonts.resolve(font)
            bold, italic = bold or fb, italic or fi
            f = _attr(family)
            parts.append(f'<w:rFonts w:ascii="{f}" w:hAnsi="{f}" w:cs="{f}" w:eastAsia="{f}"/>')
        if bold:
            parts.append('<w:b/><w:bCs/>')
        if italic:
            parts.append('<w:i/><w:iCs/>')
        caps = cp.int_(13, 0)
        if caps == 1:
            parts.append('<w:caps/>')
        elif caps == 2:
            parts.append('<w:smallCaps/>')
        strike = cp.int_(12, 0)
        if strike == 1:
            parts.append('<w:strike/>')
        elif strike >= 2:
            parts.append('<w:dstrike/>')
        color = _color_hex(cp.msg(7))
        if color and color != '000000':
            parts.append(f'<w:color w:val="{color}"/>')
        shift = cp.float_(14, 0.0) or 0.0
        if abs(shift) >= 0.25:
            parts.append(f'<w:position w:val="{round(shift * 2)}"/>')
        size = cp.float_(3)
        if size:
            parts.append(f'<w:sz w:val="{round(size * 2)}"/><w:szCs w:val="{round(size * 2)}"/>')
        underline = cp.int_(11, 0)
        if underline:
            parts.append(f'<w:u w:val="{"double" if underline == 2 else "single"}"/>')
        sup = cp.int_(10, 0)
        if sup in (1, 2):
            parts.append(f'<w:vertAlign w:val="{"superscript" if sup == 1 else "subscript"}"/>')
        return f'<w:rPr>{"".join(parts)}</w:rPr>' if parts else ''

    def ppr(self, pp: Msg, cp: Msg, style_id: int | None, numpr: str = '', jc: str | None = None,
            keep_lists_indent: bool = False) -> str:
        parts = []
        ident = self.styles.identifier(style_id)
        m = re.search(r'paragraphstyle-(Title|Subtitle|Heading \d)$', ident)
        if m and m.group(1) in HEADING_STYLES:
            sid = HEADING_STYLES[m.group(1)][0]
            self.used_heading_styles.add(m.group(1))
            parts.append(f'<w:pStyle w:val="{sid}"/>')
        if pp.bool_(10, False):
            parts.append('<w:keepNext/>')
        if pp.bool_(9, False):
            parts.append('<w:keepLines/>')
        if pp.bool_(14, False):
            parts.append('<w:pageBreakBefore/>')
        if pp.has(26) and not pp.bool_(26):
            parts.append('<w:widowControl w:val="0"/>')
        parts.append(numpr)
        tabs = []
        tabs_msg = pp.msg(25)
        for tab in (tabs_msg.msgs(1) if tabs_msg else []):
            pos = tab.float_(1)
            if pos is None:
                continue
            val = {0: 'left', 1: 'center', 2: 'right', 3: 'decimal'}.get(tab.int_(2, 0), 'left')
            tabs.append(f'<w:tab w:val="{val}" w:pos="{round(pos * PT)}"/>')
        if tabs:
            parts.append(f'<w:tabs>{"".join(tabs)}</w:tabs>')
        spacing = []
        before, after = pp.float_(21, 0.0) or 0.0, pp.float_(20, 0.0) or 0.0
        if before > 0:
            spacing.append(f'w:before="{round(before * PT)}"')
        if after > 0:
            spacing.append(f'w:after="{round(after * PT)}"')
        ls = pp.msg(13) or EMPTY
        mode, amount = ls.int_(1, 0), ls.float_(2, 1.0)
        size = cp.float_(3, 11.0) or 11.0
        line_em = self.fonts.pages_line_em(cp.str_(5))
        if mode == 0 and line_em:
            # An absolute pitch keeps Pages' pagination whatever metrics the substitute font has;
            # "at least" still lets larger inline content grow the line.
            rule = 'atLeast' if amount >= 1.0 else 'exact'
            spacing.append(f'w:line="{round(amount * size * line_em * PT)}" w:lineRule="{rule}"')
            extra = round(max(amount - 1.0, 0.0) * size * line_em, 1)
            self.line_extras[extra] = self.line_extras.get(extra, 0) + 1
        elif ls is not EMPTY:
            if mode == 0:
                spacing.append(f'w:line="{round(240 * amount)}" w:lineRule="auto"')
            elif mode == 1:
                spacing.append(f'w:line="{round(amount * PT)}" w:lineRule="atLeast"')
            elif mode in (2, 3):
                spacing.append(f'w:line="{round(amount * PT)}" w:lineRule="exact"')
            else:
                spacing.append(f'w:line="{round((size * 1.2 + amount) * PT)}" w:lineRule="atLeast"')
        if spacing:
            parts.append(f'<w:spacing {" ".join(spacing)}/>')
        if not numpr or keep_lists_indent:
            first, left, right = pp.float_(7, 0.0) or 0.0, pp.float_(11, 0.0) or 0.0, pp.float_(19, 0.0) or 0.0
            ind = []
            if left:
                ind.append(f'w:left="{round(left * PT)}"')
            if right:
                ind.append(f'w:right="{round(right * PT)}"')
            if first > 0:
                ind.append(f'w:firstLine="{round(first * PT)}"')
            elif first < 0:
                ind.append(f'w:hanging="{round(-first * PT)}"')
            if ind:
                parts.append(f'<w:ind {" ".join(ind)}/>')
        if jc is None:
            jc = {1: 'right', 2: 'center', 3: 'both'}.get(pp.int_(1, 4))
        if jc:
            parts.append(f'<w:jc w:val="{jc}"/>')
        parts.append(self.rpr(cp))
        body = ''.join(parts)
        return f'<w:pPr>{body}</w:pPr>' if body else ''

    # ----------------------------------------------------------- text

    def _runs_for_text(self, s: str, rpr: str) -> list[str]:
        out, buf = [], []

        def flush():
            if buf:
                out.append(f'<w:r>{rpr}<w:t xml:space="preserve">{_text("".join(buf))}</w:t></w:r>')
                buf.clear()

        for ch in s:
            special = None
            if ch == '\t':
                special = '<w:tab/>'
            elif ch in '\u2028\x0b':
                special = '<w:br/>'
            elif ch in '\x0c\x04':
                special = '<w:br w:type="page"/>'
            elif ch == '\x0e':
                special = '<w:br w:type="column"/>'
            elif ch == '\u00ad':
                special = '<w:softHyphen/>'
            elif ch == '\ufffc' or _INVALID_XML.match(ch):
                continue
            if special:
                flush()
                out.append(f'<w:r>{rpr}{special}</w:r>')
            else:
                buf.append(ch)
        flush()
        return out

    def render_storage(self, sid: int, ctx: Context) -> list[str]:
        """Render a TSWP.StorageArchive to a list of docx block elements (paragraphs, tables)."""
        # Drawables point back at the storage they are anchored in; never render it twice.
        if sid in self._rendering:
            return []
        self._rendering.add(sid)
        try:
            return self._render_storage(sid, ctx)
        finally:
            self._rendering.discard(sid)

    def _render_storage(self, sid: int, ctx: Context) -> list[str]:
        st = self.pkg.get(sid)
        text = ''.join(st.strs(3))
        to_py = _utf16_index_converter(text)

        def table(fn: int, kind: str = 'object') -> AttrTable:
            return AttrTable(st.msg(fn), kind, to_py)

        para_styles, para_levels = table(5), table(6, 'pair')
        list_styles, char_styles = table(7), table(8)
        attachments, smartfields = table(9), table(11)
        list_starts = table(14, 'pair')

        blocks: list[str] = []
        starts = [0] + [m.end() for m in re.finditer('[\n\u2029\x0c\x04\x0e]', text)]
        for i, ps in enumerate(starts):
            pe = starts[i + 1] if i + 1 < len(starts) else len(text)
            # Keep page/column breaks (they end the paragraph in Pages), drop plain paragraph marks.
            if pe > ps and text[pe - 1] in '\n\u2029':
                content_end = pe - 1
            else:
                content_end = pe
            blocks.extend(self._render_paragraph(
                text, ps, content_end, ctx, para_styles, para_levels, list_styles, char_styles,
                attachments, smartfields, list_starts))
        return blocks

    def _render_paragraph(self, text, ps, pe, ctx, para_styles, para_levels, list_styles,
                          char_styles, attachments, smartfields, list_starts) -> list[str]:
        style_id = para_styles.at_inherit(ps)
        pp = self.styles.props(style_id, 12)
        cp = self.styles.props(style_id, 11)

        numpr = ''
        ls_id = list_styles.at_inherit(ps) if list_styles else pp.ref(40)
        level_entry = para_levels.at(ps)
        level = min(max(level_entry[0] if level_entry else 0, 0), 8)
        if ctx.part != 'header' and self.numbering.is_list(ls_id, level):
            start_entry = list_starts.exact(ps)
            restart = bool(start_entry and start_entry[0])
            start = start_entry[1] if start_entry and start_entry[1] > 0 else 1
            num = self.numbering.num_id(ls_id, restart, start)
            numpr = f'<w:numPr><w:ilvl w:val="{level}"/><w:numId w:val="{num}"/></w:numPr>'

        bounds = {ps, pe}
        for tbl in (char_styles, smartfields):
            bounds.update(tbl.boundaries(ps, pe))
        for a in attachments.boundaries(ps - 1, pe):
            bounds.update((a, a + 1))
        bounds = sorted(b for b in bounds if ps <= b <= pe)

        pieces: list[list[str]] = [[]]     # runs between block-level attachments
        block_after: list[list[str]] = []  # blocks following each piece
        link_runs: list[str] = []
        link_url: str | None = None

        def close_link():
            nonlocal link_url
            if link_runs:
                if link_url and ctx.rels_allowed:
                    rid = self._hyperlink_rel(link_url)
                    pieces[-1].append(f'<w:hyperlink r:id="{rid}" w:history="1">{"".join(link_runs)}</w:hyperlink>')
                else:
                    pieces[-1].extend(link_runs)
                link_runs.clear()
            link_url = None

        for a, b in zip(bounds, bounds[1:]):
            if a >= b:
                continue
            cs_id = char_styles.at(a)
            run_props = merge_props(cp, self.styles.props(cs_id, 11)) if cs_id else cp
            rpr = self.rpr(run_props)
            url = None
            field_id = smartfields.at(a)
            if field_id is not None and self.pkg.type(field_id) == T_HYPERLINK:
                url = self.pkg.get(field_id).str_(2)
            if url != link_url:
                close_link()
                link_url = url
            segment = text[a:b]
            if segment == '\ufffc':
                att = attachments.exact(a)
                inline, blocks = self._attachment(att, ctx, rpr)
                if blocks:
                    close_link()
                    block_after.append(blocks)
                    pieces.append([])
                    link_url = url
                elif inline:
                    link_runs.extend(inline)
                continue
            link_runs.extend(self._runs_for_text(segment, rpr))
        close_link()

        if not block_after:
            return [f'<w:p>{self.ppr(pp, cp, style_id, numpr)}{"".join(pieces[0])}</w:p>']
        out = []
        for idx, runs in enumerate(pieces):
            if runs:
                out.append(f'<w:p>{self.ppr(pp, cp, style_id, numpr)}{"".join(runs)}</w:p>')
                numpr = ''
            if idx < len(block_after):
                out.extend(block_after[idx])
        return out

    # ----------------------------------------------------------- attachments

    def _attachment(self, att_id: int | None, ctx: Context, rpr: str) -> tuple[list[str], list[str]]:
        """Return ``(inline_runs, blocks)`` for an attachment character."""
        if att_id is None:
            return [], []
        try:
            return self._attachment_unguarded(att_id, ctx, rpr)
        except Exception:  # noqa: BLE001 - schema-less decoding: lose one object, not the document
            self.report.skip('Objekt mit Lesefehler')
            return [], []

    def _attachment_unguarded(self, att_id: int, ctx: Context, rpr: str) -> tuple[list[str], list[str]]:
        typ = self.pkg.type(att_id)
        if typ == T_DRAWABLE_ATTACHMENT:
            return self._drawable(self.pkg.get(att_id).ref(1), ctx, 0)
        if ctx.part == 'header':
            # Header/footer attachments that are not drawables are page number fields.
            return [f'<w:fldSimple w:instr=" PAGE "><w:r>{rpr}<w:t>1</w:t></w:r></w:fldSimple>'], []
        self.report.skip(f'Typ {typ}')
        return [], []

    def _drawable(self, did: int | None, ctx: Context, depth: int) -> tuple[list[str], list[str]]:
        typ = self.pkg.type(did)
        m = self.pkg.get(did)
        if typ == T_TABLE_INFO:
            if ctx.part == 'body':
                return [], [self.render_table(did, ctx)]
            self.report.skip('Tabelle in Tabelle/Kopfzeile')
            return [], []
        image = self._image_run(m, ctx)
        if image:
            return [image], []
        storages = [r for fn in sorted(m.fields) for r in self._refs_in(m, fn)
                    if self.pkg.type(r) == T_STORAGE]
        if storages:
            blocks = []
            for sid in storages:
                blocks.extend(self.render_storage(sid, ctx))
            return [], blocks
        if depth < 5:
            inline, blocks = [], []
            for child in self._refs_in(m, 2):
                ci, cb = self._drawable(child, ctx, depth + 1)
                inline.extend(ci)
                blocks.extend(cb)
            if inline or blocks:
                return inline, blocks
        self.report.skip('Objekt' if typ is None else f'Objekt {typ}')
        return [], []

    @staticmethod
    def _refs_in(m: Msg, fn: int) -> list[int]:
        out = []
        for sub in m.msgs(fn):
            if len(sub.fields) == 1 and sub.int_(1) is not None:
                out.append(sub.int_(1))
        return out

    def _image_run(self, m: Msg, ctx: Context) -> str | None:
        # Field order: data, original data, legacy data; thumbnails are only a last resort.
        name = None
        for fn in (11, 13, 2, 15, 12):
            sub = m.msg(fn)
            if sub and sub.int_(1) in self.pkg.data_map:
                name = self.pkg.data_map[sub.int_(1)]
                break
        if name is None:
            return None
        if not ctx.rels_allowed:
            self.report.skip('Bild in Kopf-/Fußzeile')
            return ''
        data = self.pkg.read_data(name)
        info = _image_info(data)
        if info is None:
            data = _convert_image_with_magick(data)
            info = _image_info(data) if data else None
        if info is None:
            self.report.skip('Bildformat')
            return ''
        ext, px_w, px_h = info
        w_pt = h_pt = None
        geom = _find_geometry(m)
        if geom:
            size = geom.msg(2)
            w_pt, h_pt = size.float_(1), size.float_(2)
        if not w_pt or not h_pt:
            w_pt, h_pt = (px_w or 300) * 0.75, (px_h or 200) * 0.75
        if w_pt > ctx.max_width_pt:
            h_pt *= ctx.max_width_pt / w_pt
            w_pt = ctx.max_width_pt
        self._drawing_id += 1
        n = self._drawing_id
        target = f'media/image{n}.{ext}'
        self.media.append((target, data))
        rid = self._add_rel('image', target)
        cx, cy = round(w_pt * EMU_PER_PT), round(h_pt * EMU_PER_PT)
        return (f'<w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0">'
                f'<wp:extent cx="{cx}" cy="{cy}"/><wp:docPr id="{n}" name="Image {n}"/>'
                f'<wp:cNvGraphicFramePr><a:graphicFrameLocks noChangeAspect="1"/></wp:cNvGraphicFramePr>'
                f'<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture">'
                f'<pic:pic><pic:nvPicPr><pic:cNvPr id="{n}" name="image{n}.{ext}"/><pic:cNvPicPr/></pic:nvPicPr>'
                f'<pic:blipFill><a:blip r:embed="{rid}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>'
                f'<pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm>'
                f'<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic>'
                f'</a:graphicData></a:graphic></wp:inline></w:drawing></w:r>')

    # ----------------------------------------------------------- tables

    def render_table(self, table_info_id: int, ctx: Context) -> str:
        """Render a TST.TableInfoArchive as a docx table."""
        model = self.pkg.get(self.pkg.get(table_info_id).ref(2))
        nrows, ncols = model.int_(6, 0), model.int_(7, 0)
        header_rows, header_cols = model.int_(9, 0), model.int_(10, 0)
        footer_rows = model.int_(11, 0)
        store = model.msg(4) or EMPTY

        lists: dict[int, dict[int, Msg]] = {}
        for fn in store.fields:
            for rid in self._refs_in(store, fn):
                if self.pkg.type(rid) == T_DATA_LIST:
                    dl = self.pkg.get(rid)
                    lists.setdefault(dl.int_(1, 0), {}).update({e.int_(1): e for e in dl.msgs(3)})
        strings, rich = lists.get(1, {}), lists.get(8, {})

        col_sizes, col_hidden = self._header_sizes([store.ref(2)])
        row_hdr = store.msg(1)
        row_sizes, row_hidden = self._header_sizes(row_hdr.refs(2) if row_hdr else [])

        cells: dict[tuple[int, int], tuple] = {}
        tiles = store.msg(3) or EMPTY
        tile_size = tiles.int_(2, 256) or 256
        for entry in tiles.msgs(1):
            tile = self.pkg.get(entry.ref(2))
            base_row = (entry.int_(1, 0) or 0) * tile_size
            for row_info in tile.msgs(5):
                row = base_row + row_info.int_(1, 0)
                storage = next((v for wt, v in row_info.fields.get(6, ()) if wt == 2), None)
                offsets = next((v for wt, v in row_info.fields.get(7, ()) if wt == 2), None)
                if storage is None or offsets is None:
                    continue
                wide = row_info.bool_(8, False)
                for col, off in enumerate(struct.unpack(f'<{len(offsets) // 2}H', offsets)):
                    if off == 0xFFFF:
                        continue
                    cell = _parse_cell(storage, off * 4 if wide else off)
                    if cell:
                        cells[(row, col)] = cell

        visible_cols = [c for c in range(ncols) if not col_hidden.get(c)]
        visible_rows = [r for r in range(nrows) if not row_hidden.get(r)]
        default_w = model.float_(17, 98.0) or 98.0
        widths = [col_sizes.get(c) or default_w for c in visible_cols]
        total = sum(widths) or 1.0
        if total > ctx.max_width_pt:
            widths = [w * ctx.max_width_pt / total for w in widths]
        tw = [max(round(w * PT), 200) for w in widths]

        border = '<w:{0} w:val="single" w:sz="4" w:space="0" w:color="000000"/>'
        borders = ''.join(border.format(s) for s in ('top', 'left', 'bottom', 'right', 'insideH', 'insideV'))
        grid = ''.join('<w:gridCol w:w="%d"/>' % w for w in tw)
        out = [f'<w:tbl><w:tblPr><w:tblW w:w="{sum(tw)}" w:type="dxa"/><w:tblLayout w:type="fixed"/>'
               f'<w:tblBorders>{borders}</w:tblBorders>'
               f'<w:tblCellMar><w:top w:w="60" w:type="dxa"/><w:left w:w="100" w:type="dxa"/>'
               f'<w:bottom w:w="60" w:type="dxa"/><w:right w:w="100" w:type="dxa"/></w:tblCellMar>'
               f'<w:tblLook w:val="0000"/></w:tblPr>'
               f'<w:tblGrid>{grid}</w:tblGrid>']
        cell_ctx = Context('cell', max_width_pt=max(widths) if widths else ctx.max_width_pt)
        for r in visible_rows:
            if r < header_rows:
                style_ref = model.ref(25)
            elif r >= nrows - footer_rows:
                style_ref = model.ref(27)
            else:
                style_ref = model.ref(24)
            tr = ['<w:tr>']
            if r < header_rows:
                tr.append('<w:trPr><w:tblHeader/></w:trPr>')
            elif row_sizes.get(r):
                tr.append(f'<w:trPr><w:trHeight w:val="{round(row_sizes[r] * PT)}"/></w:trPr>')
            for c, w in zip(visible_cols, tw):
                cell_style = model.ref(26) if c < header_cols and r >= header_rows else style_ref
                content = self._cell_blocks(cells.get((r, c)), strings, rich, cell_style, cell_ctx)
                tr.append(f'<w:tc><w:tcPr><w:tcW w:w="{w}" w:type="dxa"/></w:tcPr>{"".join(content)}</w:tc>')
            tr.append('</w:tr>')
            out.append(''.join(tr))
        out.append('</w:tbl>')
        return ''.join(out)

    def _header_sizes(self, bucket_ids: list[int | None]) -> tuple[dict[int, float], dict[int, bool]]:
        sizes, hidden = {}, {}
        for bid in bucket_ids:
            for h in self.pkg.get(bid).msgs(2):
                idx = h.int_(1)
                if idx is None:
                    continue
                if h.float_(2):
                    sizes[idx] = h.float_(2)
                hidden[idx] = bool(h.int_(3, 0))
        return sizes, hidden

    def _cell_blocks(self, cell, strings, rich, style_id, ctx) -> list[str]:
        pp, cp = self.styles.props(style_id, 12), self.styles.props(style_id, 11)
        blocks: list[str] = []
        if cell:
            ctype, number, seconds, string_id, rich_id = cell
            if ctype == 9 and rich_id in rich:
                entry = rich[rich_id]
                payload = next((r for fn in sorted(entry.fields) for r in self._refs_in(entry, fn)), None)
                storage = self.pkg.get(payload).ref(1)
                if self.pkg.type(storage) == T_STORAGE:
                    blocks = self.render_storage(storage, ctx)
            else:
                value = self._cell_text(ctype, number, seconds, strings.get(string_id))
                if value:
                    rpr = self.rpr(cp)
                    for line in value.split('\n'):
                        runs = ''.join(self._runs_for_text(line, rpr))
                        blocks.append(f'<w:p>{self.ppr(pp, cp, style_id)}{runs}</w:p>')
        if not blocks or not blocks[-1].startswith('<w:p'):
            # A table cell must end with a paragraph.
            blocks.append(f'<w:p>{self.ppr(pp, cp, style_id)}</w:p>')
        return blocks

    def _cell_text(self, ctype, number, seconds, string_entry) -> str:
        if ctype == 3:
            return string_entry.str_(3, '') if string_entry else ''
        if ctype in (2, 10) and number is not None:
            if isinstance(number, decimal.Decimal):
                s = format(number.normalize(), 'f')
            else:
                s = f'{number:.15g}'
            if '.' in s:
                s = s.rstrip('0').rstrip('.')
            return s.replace('.', ',') if self.decimal_comma else s
        if ctype == 5 and seconds is not None:
            dt = APPLE_EPOCH + datetime.timedelta(seconds=seconds)
            return dt.strftime('%d.%m.%Y %H:%M' if dt.hour or dt.minute else '%d.%m.%Y')
        if ctype == 6 and number is not None:
            return 'WAHR' if number else 'FALSCH'
        if ctype == 7 and number is not None:
            total = int(round(number))
            return f'{total // 3600}:{total % 3600 // 60:02d}:{total % 60:02d}'
        if ctype == 8:
            return '#FEHLER'
        return ''

    # ----------------------------------------------------------- document

    def build(self) -> dict[str, bytes]:
        """Assemble all docx parts, keyed by their path inside the package."""
        doc = self._find_document()
        page_w, page_h = doc.float_(30, 595.28) or 595.28, doc.float_(31, 841.89) or 841.89
        margins = [doc.float_(fn, 56.69) for fn in (32, 33, 34, 35)]
        left, right, top, bottom = [m if m is not None else 56.69 for m in margins]
        header_m, footer_m = doc.float_(36, 35.0) or 35.0, doc.float_(37, 35.0) or 35.0
        text_width = max(page_w - left - right, 100.0)
        locale = (doc.msg(15).msg(1).str_(4, '') if doc.msg(15) and doc.msg(15).msg(1) else '') or ''
        self.decimal_comma = not locale.startswith(('en', 'ja', 'zh', 'ko', 'he', 'th'))

        body_id = doc.ref(4)
        ctx = Context('body', max_width_pt=text_width)
        if self.pkg.type(body_id) == T_STORAGE:
            blocks = self.render_storage(body_id, ctx)
        else:
            self.report.notes.append(MESSAGES['no_body'])
            blocks = []
            for sid in self.pkg.ids_of_type(T_STORAGE):
                if self.pkg.get(sid).int_(1) == 3 and ''.join(self.pkg.get(sid).strs(3)).strip():
                    blocks.extend(self.render_storage(sid, ctx))
        if not blocks or not blocks[-1].startswith('<w:p'):
            blocks.append('<w:p/>')
        # Pages puts the extra leading of spaced lines below each line and does not need it
        # after the last line of a page; Word puts it above. Raising the top margin by that
        # amount keeps both the line positions and the lines per page equal to Pages.
        if self.line_extras:
            top = max(top - max(self.line_extras, key=self.line_extras.get), 0.0)

        parts: dict[str, bytes] = {}
        sect = []
        try:
            header_xml, footer_xml = self._header_footer(body_id, text_width)
        except Exception:  # noqa: BLE001 - headers are optional; keep the body
            self.report.skip('Kopf-/Fußzeile')
            header_xml = footer_xml = None
        if header_xml:
            parts['word/header1.xml'] = header_xml.encode('utf-8')
            sect.append(f'<w:headerReference w:type="default" r:id="{self._add_rel("header", "header1.xml")}"/>')
        if footer_xml:
            parts['word/footer1.xml'] = footer_xml.encode('utf-8')
            sect.append(f'<w:footerReference w:type="default" r:id="{self._add_rel("footer", "footer1.xml")}"/>')
        orient = ' w:orient="landscape"' if page_w > page_h else ''
        sect.append(f'<w:pgSz w:w="{round(page_w * PT)}" w:h="{round(page_h * PT)}"{orient}/>')
        sect.append(f'<w:pgMar w:top="{round(top * PT)}" w:right="{round(right * PT)}" '
                    f'w:bottom="{round(bottom * PT)}" w:left="{round(left * PT)}" '
                    f'w:header="{round(header_m * PT)}" w:footer="{round(footer_m * PT)}" w:gutter="0"/>')
        document = (f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
                    f'<w:document {NS}><w:body>{"".join(blocks)}<w:sectPr>{"".join(sect)}</w:sectPr>'
                    f'</w:body></w:document>')
        parts['word/document.xml'] = document.encode('utf-8')

        base_style = self._body_style_id(body_id)
        parts['word/styles.xml'] = self._styles_xml(base_style).encode('utf-8')
        parts['word/settings.xml'] = SETTINGS_XML.encode('utf-8')
        self.rels.insert(0, ('rIdStyles', f'{REL_BASE}/styles', 'styles.xml', False))
        self.rels.insert(1, ('rIdSettings', f'{REL_BASE}/settings', 'settings.xml', False))
        if self.numbering.nums:
            parts['word/numbering.xml'] = self.numbering.xml().encode('utf-8')
            self.rels.insert(2, ('rIdNumbering', f'{REL_BASE}/numbering', 'numbering.xml', False))
        for target, data in self.media:
            parts[f'word/{target}'] = data

        external = ' TargetMode="External"'
        rels = ''.join(
            f'<Relationship Id="{rid}" Type="{t}" Target="{_attr(target)}"{external if ext else ""}/>'
            for rid, t, target, ext in self.rels)
        parts['word/_rels/document.xml.rels'] = (
            '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
            '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
            f'{rels}</Relationships>').encode('utf-8')
        parts['_rels/.rels'] = ROOT_RELS.encode('utf-8')
        parts['docProps/core.xml'] = self._core_xml().encode('utf-8')
        parts['[Content_Types].xml'] = _content_types(parts).encode('utf-8')
        return parts

    def _find_document(self) -> Msg:
        for typ in (10000,):
            ids = self.pkg.ids_of_type(typ)
            if ids:
                return self.pkg.get(ids[0])
        raise PagesError(MESSAGES['not_pages'].format(path=''))

    def _body_style_id(self, body_id) -> int | None:
        """The template's body text style, which supplies Word's document defaults."""
        for oid in self.pkg.ids_of_type(2022):
            sup = self.pkg.get(oid).msg(1)
            if sup and re.search(r'paragraphstyle-Body( 1)?$', sup.str_(2) or ''):
                return oid
        return AttrTable(self.pkg.get(body_id).msg(5), 'object', None).at_inherit(0)

    def _header_footer(self, body_id, text_width) -> tuple[str | None, str | None]:
        st = self.pkg.get(body_id)
        section_id = AttrTable(st.msg(17), 'object', None).at_inherit(0)
        section = self.pkg.get(section_id)
        ctx = Context('header', max_width_pt=text_width)
        result = []
        for part_field, root in ((1, 'hdr'), (2, 'ftr')):
            xml = None
            for master_field in (23, 24, 25):
                master = self.pkg.get(section.ref(master_field))
                storages = master.refs(part_field)
                rendered = [self.render_storage(s, ctx) if self._has_content(s) else [] for s in storages]
                if any(rendered):
                    xml = self._compose_header(rendered, text_width, root)
                    break
            result.append(xml)
        return result[0], result[1]

    def _has_content(self, sid) -> bool:
        st = self.pkg.get(sid)
        return bool(''.join(st.strs(3)).strip()) or bool(st.msg(9) and st.msg(9).msgs(1))

    def _compose_header(self, parts: list[list[str]], text_width: float, root: str) -> str:
        filled = [(i, p) for i, p in enumerate(parts) if p]
        if len(filled) == 1:
            idx, blocks = filled[0]
            jc = ('left', 'center', 'right')[idx] if len(parts) == 3 else None
            if jc and jc != 'left':
                blocks = [re.sub(r'<w:jc w:val="[^"]*"/>', '', b) for b in blocks]
                blocks = [b.replace('<w:pPr>', f'<w:pPr><w:jc w:val="{jc}"/>', 1) if '<w:pPr>' in b
                          else b.replace('<w:p>', f'<w:p><w:pPr><w:jc w:val="{jc}"/></w:pPr>', 1)
                          for b in blocks]
            body = ''.join(blocks)
        else:
            # Pages headers have left, centre and right fields; Word gets one line with tab stops.
            runs = []
            for i, blocks in enumerate(parts):
                if i:
                    runs.append('<w:r><w:tab/></w:r>')
                if blocks:
                    m = re.search(r'</w:pPr>(.*)</w:p>$', blocks[0]) or re.search(r'<w:p>(.*)</w:p>$', blocks[0])
                    runs.append(m.group(1) if m else '')
            tabs = (f'<w:tabs><w:tab w:val="center" w:pos="{round(text_width * PT / 2)}"/>'
                    f'<w:tab w:val="right" w:pos="{round(text_width * PT)}"/></w:tabs>')
            body = f'<w:p><w:pPr>{tabs}</w:pPr>{"".join(runs)}</w:p>'
        return f'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<w:{root} {NS}>{body}</w:{root}>'

    def _styles_xml(self, base_style_id) -> str:
        cp = self.styles.props(base_style_id, 11)
        font = cp.str_(5) or 'Helvetica Neue'
        family = _attr(self.fonts.resolve(font)[0])
        size = round((cp.float_(3, 11.0) or 11.0) * 2)
        styles = [
            '<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style>',
            '<w:style w:type="character" w:default="1" w:styleId="DefaultParagraphFont">'
            '<w:name w:val="Default Paragraph Font"/><w:uiPriority w:val="1"/><w:semiHidden/></w:style>',
            '<w:style w:type="table" w:default="1" w:styleId="TableNormal"><w:name w:val="Normal Table"/>'
            '<w:tblPr><w:tblInd w:w="0" w:type="dxa"/><w:tblCellMar><w:top w:w="0" w:type="dxa"/>'
            '<w:left w:w="108" w:type="dxa"/><w:bottom w:w="0" w:type="dxa"/><w:right w:w="108" w:type="dxa"/>'
            '</w:tblCellMar></w:tblPr></w:style>',
        ]
        for key in self.used_heading_styles:
            sid, name, outline = HEADING_STYLES[key]
            lvl = f'<w:outlineLvl w:val="{outline}"/>' if outline is not None else ''
            styles.append(f'<w:style w:type="paragraph" w:styleId="{sid}"><w:name w:val="{name}"/>'
                          f'<w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/>'
                          f'<w:pPr><w:keepNext/>{lvl}</w:pPr></w:style>')
        return ('<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
                f'<w:styles {NS}><w:docDefaults><w:rPrDefault><w:rPr>'
                f'<w:rFonts w:ascii="{family}" w:hAnsi="{family}" w:cs="{family}" w:eastAsia="{family}"/>'
                f'<w:sz w:val="{size}"/><w:szCs w:val="{size}"/></w:rPr></w:rPrDefault>'
                '<w:pPrDefault><w:pPr><w:widowControl/><w:spacing w:after="0" w:line="240" w:lineRule="auto"/>'
                f'</w:pPr></w:pPrDefault></w:docDefaults>{"".join(styles)}</w:styles>')

    def _core_xml(self) -> str:
        now = datetime.datetime.now(datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')
        return ('<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
                '<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" '
                'xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" '
                'xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">'
                f'<dc:title>{_text(self.title)}</dc:title>'
                '<dc:description>Converted from Apple Pages with pages2docx</dc:description>'
                f'<dcterms:created xsi:type="dcterms:W3CDTF">{now}</dcterms:created>'
                f'<dcterms:modified xsi:type="dcterms:W3CDTF">{now}</dcterms:modified>'
                '</cp:coreProperties>')

    title = ''


class AttrTable:
    """Character-indexed attribute runs of a TSWP storage (Object/ParaData attribute tables)."""

    def __init__(self, m: Msg | None, kind: str, to_py):
        rows = []
        for e in (m.msgs(1) if m else []):
            idx = e.int_(1, 0)
            if to_py:
                idx = to_py(idx)
            if kind == 'pair':
                val = (e.int_(2, 0), e.int_(3, 0))
            else:
                val = e.ref(2)
            rows.append((idx, val))
        rows.sort(key=lambda r: r[0])
        self.idx = [r[0] for r in rows]
        self.vals = [r[1] for r in rows]

    def __bool__(self) -> bool:
        return bool(self.idx)

    def at(self, i: int):
        k = bisect.bisect_right(self.idx, i) - 1
        return self.vals[k] if k >= 0 else None

    def at_inherit(self, i: int):
        """Value at ``i``; entries without a value continue the previous one (paragraph styles)."""
        k = bisect.bisect_right(self.idx, i) - 1
        while k >= 0 and self.vals[k] is None:
            k -= 1
        return self.vals[k] if k >= 0 else None

    def exact(self, i: int):
        k = bisect.bisect_left(self.idx, i)
        if k < len(self.idx) and self.idx[k] == i:
            return self.vals[k]
        return None

    def boundaries(self, lo: int, hi: int) -> list[int]:
        return [x for x in self.idx if lo < x < hi]


def _utf16_index_converter(text: str):
    """Pages indexes text in UTF-16 code units; Python strings index code points."""
    if all(ord(c) < 0x10000 for c in text):
        return None
    mapping = []
    for i, c in enumerate(text):
        mapping.append(i)
        if ord(c) >= 0x10000:
            mapping.append(i + 1)
    mapping.append(len(text))
    return lambda idx: mapping[idx] if idx < len(mapping) else len(text)


def _find_geometry(m: Msg) -> Msg | None:
    """Locate TSD.GeometryArchive (position + size) by descending the ``super`` chain."""
    cur = m
    for _ in range(4):
        cur = cur.msg(1)
        if cur is None:
            return None
        size = cur.msg(2)
        pos = cur.msg(1)
        if size and pos and size.fields.get(1, [(0,)])[0][0] == 5 and size.fields.get(2, [(0,)])[0][0] == 5:
            return cur
    return None


def _decimal128(b: bytes) -> decimal.Decimal:
    exp = (((b[15] & 0x7F) << 7) | (b[14] >> 1)) - 0x1820
    mantissa = b[14] & 1
    for i in range(13, -1, -1):
        mantissa = mantissa * 256 + b[i]
    value = decimal.Decimal(mantissa).scaleb(exp)
    return -value if b[15] & 0x80 else value


def _parse_cell(buf: bytes, off: int):
    """Decode a TST cell (storage format version 5): (type, number, seconds, string_id, rich_id)."""
    if off + 12 > len(buf) or buf[off] != 5:
        return None
    ctype = buf[off + 1]
    flags = struct.unpack_from('<I', buf, off + 8)[0]
    p = off + 12
    number = seconds = string_id = rich_id = None
    try:
        if flags & 0x1:
            number = _decimal128(buf[p:p + 16])
            p += 16
        if flags & 0x2:
            number = struct.unpack_from('<d', buf, p)[0]
            p += 8
        if flags & 0x4:
            seconds = struct.unpack_from('<d', buf, p)[0]
            p += 8
        if flags & 0x8:
            string_id = struct.unpack_from('<i', buf, p)[0]
            p += 4
        if flags & 0x10:
            rich_id = struct.unpack_from('<i', buf, p)[0]
    except struct.error:
        return None
    return ctype, number, seconds, string_id, rich_id


SETTINGS_XML = ('<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
                '<w:settings xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">'
                '<w:defaultTabStop w:val="720"/><w:characterSpacingControl w:val="doNotCompress"/>'
                '<w:compat><w:compatSetting w:name="compatibilityMode" '
                'w:uri="http://schemas.microsoft.com/office/word" w:val="15"/></w:compat></w:settings>')

ROOT_RELS = ('<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
             '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
             '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/'
             'relationships/officeDocument" Target="word/document.xml"/>'
             '<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/'
             'metadata/core-properties" Target="docProps/core.xml"/></Relationships>')


def _content_types(parts: dict[str, bytes]) -> str:
    wml = 'application/vnd.openxmlformats-officedocument.wordprocessingml'
    overrides = {
        'word/document.xml': f'{wml}.document.main+xml',
        'word/styles.xml': f'{wml}.styles+xml',
        'word/settings.xml': f'{wml}.settings+xml',
        'word/numbering.xml': f'{wml}.numbering+xml',
        'word/header1.xml': f'{wml}.header+xml',
        'word/footer1.xml': f'{wml}.footer+xml',
        'docProps/core.xml': 'application/vnd.openxmlformats-package.core-properties+xml',
    }
    defaults = {'rels': 'application/vnd.openxmlformats-package.relationships+xml', 'xml': 'application/xml',
                'png': 'image/png', 'jpeg': 'image/jpeg', 'gif': 'image/gif', 'bmp': 'image/bmp',
                'tiff': 'image/tiff'}
    xml = ['<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
           '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">']
    xml += [f'<Default Extension="{e}" ContentType="{t}"/>' for e, t in defaults.items()]
    xml += [f'<Override PartName="/{p}" ContentType="{t}"/>' for p, t in overrides.items() if p in parts]
    xml.append('</Types>')
    return ''.join(xml)


# --------------------------------------------------------------------------- entry points

def convert(src: str, dst: str) -> Report:
    """Convert one .pages document to .docx.

    Args:
        src: Path to a .pages file (zip) or .pages bundle directory.
        dst: Path of the .docx to write; an existing file is replaced.

    Returns:
        What could not be carried over.

    Raises:
        PagesError: If the input is not a supported Pages document.
    """
    pkg = PagesPackage(src)
    conv = Converter(pkg)
    conv.title = os.path.splitext(os.path.basename(src.rstrip('/\\')))[0]
    parts = conv.build()
    tmp = dst + '.tmp'
    with zipfile.ZipFile(tmp, 'w', zipfile.ZIP_DEFLATED) as z:
        # [Content_Types].xml first: some readers sniff the package by its first entry.
        for name in ['[Content_Types].xml'] + [n for n in parts if n != '[Content_Types].xml']:
            z.writestr(name, parts[name])
    os.replace(tmp, dst)
    return conv.report


def default_output(src: str) -> str:
    """``X.pages`` -> ``X.docx`` next to it, numbered if that name is taken."""
    stem = os.path.splitext(src.rstrip('/\\'))[0]
    candidate = stem + '.docx'
    n = 2
    while os.path.exists(candidate):
        candidate = f'{stem} ({n}).docx'
        n += 1
    return candidate


def find_wordcraft() -> str | None:
    override = os.environ.get('WORDCRAFT_EXE')
    for path in ([override] if override else []) + WORDCRAFT_CANDIDATES:
        if path and os.path.isfile(path):
            return path
    return None


def open_document(path: str) -> None:
    exe = find_wordcraft()
    if exe:
        subprocess.Popen([exe, path], close_fds=True,
                         creationflags=getattr(subprocess, 'DETACHED_PROCESS', 0))
    elif hasattr(os, 'startfile'):
        os.startfile(path)


def _show_error(text: str) -> None:
    if sys.platform == 'win32':
        import ctypes
        ctypes.windll.user32.MessageBoxW(None, text, MESSAGES['title'], 0x10)
    else:
        print(text, file=sys.stderr)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description='Convert Apple Pages documents to Word (.docx).')
    parser.add_argument('inputs', nargs='+', help='.pages files')
    parser.add_argument('-o', '--output', help='output .docx (single input only)')
    parser.add_argument('--open', action='store_true', help='open the result in WordCraft')
    parser.add_argument('--overwrite', action='store_true', help='replace an existing .docx of the same name')
    args = parser.parse_args(argv)
    gui = sys.stdout is None or os.path.basename(sys.executable).lower().startswith('pythonw')

    def say(text: str) -> None:
        if not gui:
            print(text)

    if args.output and len(args.inputs) != 1:
        parser.error(MESSAGES['usage_output'])
    status = 0
    for src in args.inputs:
        stem_docx = os.path.splitext(src.rstrip('/\\'))[0] + '.docx'
        if args.output:
            dst = args.output
        elif args.overwrite:
            dst = stem_docx
        elif args.open and os.path.exists(stem_docx) and os.path.getmtime(stem_docx) >= os.path.getmtime(src):
            # Already converted (and possibly edited since): open that instead of piling up copies.
            say(MESSAGES['reused'].format(path=stem_docx))
            open_document(stem_docx)
            continue
        else:
            dst = default_output(src)
        try:
            report = convert(src, dst)
        except Exception as e:  # noqa: BLE001 - any decoder failure must reach the user, also without a console
            if isinstance(e, PagesError):
                msg = str(e)
            else:
                msg = MESSAGES['failed'].format(path=src, error=f'{type(e).__name__}: {e}')
            if gui:
                _show_error(msg)
            say(msg)
            status = 1
            continue
        say(MESSAGES['saved'].format(path=dst))
        for note in report.notes:
            say(note)
        if report.skipped:
            kinds = ', '.join(f'{k}: {v}' for k, v in report.skipped.items())
            say(MESSAGES['skipped'].format(count=sum(report.skipped.values()), kinds=kinds))
        if args.open:
            open_document(dst)
    return status


if __name__ == '__main__':
    sys.exit(main())
