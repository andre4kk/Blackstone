"""Lettore NBT di rete (little-endian con varint, come i file di Dragonfly) e scrittore NBT su disco (little-endian), con i tipi preservati.

Modulo di appoggio di extract_blocks.py ed extract_creative.py: Byte, Short e Long sono sottoclassi di int (Int e' un int semplice),
cosi' un valore riscritto mantiene il tipo di tag che aveva in lettura.
"""
import struct


class Byte(int):
    pass


class Short(int):
    pass


class Long(int):
    pass


class Float(float):
    pass


class List(list):
    """Lista NBT: `elem` e' il tipo dei suoi elementi (serve anche alle liste vuote)."""

    def __init__(self, elem, items=()):
        super().__init__(items)
        self.elem = elem


class Reader:
    def __init__(self, data):
        self.b = data
        self.p = 0

    def done(self):
        return self.p >= len(self.b)

    def take(self, n):
        if n < 0 or self.p + n > len(self.b):
            raise ValueError("NBT troncato")
        v = self.b[self.p : self.p + n]
        self.p += n
        return v

    def varint(self):
        v = shift = 0
        while True:
            c = self.take(1)[0]
            v |= (c & 0x7F) << shift
            shift += 7
            if not c & 0x80:
                return v
            if shift > 70:
                raise ValueError("varint troppo lungo")

    def zigzag(self):
        v = self.varint()
        return (v >> 1) ^ -(v & 1)

    def string(self):
        return self.take(self.varint()).decode("utf-8")

    def payload(self, t):
        if t == 1:
            return Byte(struct.unpack("<b", self.take(1))[0])
        if t == 2:
            return Short(struct.unpack("<h", self.take(2))[0])
        if t == 3:
            return self.zigzag()
        if t == 4:
            return Long(self.zigzag())
        if t == 5:
            return Float(struct.unpack("<f", self.take(4))[0])
        if t == 6:
            return struct.unpack("<d", self.take(8))[0]
        if t == 7:
            return bytes(self.take(self.zigzag()))
        if t == 8:
            return self.string()
        if t == 9:
            elem = self.take(1)[0]
            return List(elem, [self.payload(elem) for _ in range(self.zigzag())])
        if t == 10:
            d = {}
            while True:
                tt = self.take(1)[0]
                if tt == 0:
                    return d
                key = self.string()
                d[key] = self.payload(tt)
        raise ValueError(f"tipo NBT {t} non gestito")

    def root(self):
        """Legge una radice (tipo, nome, valore) e avanza oltre di essa."""
        t = self.take(1)[0]
        return t, self.string(), self.payload(t)


def tag_id(v):
    if isinstance(v, bool):
        raise TypeError("bool non ammesso: usare Byte")
    if isinstance(v, Byte):
        return 1
    if isinstance(v, Short):
        return 2
    if isinstance(v, Long):
        return 4
    if isinstance(v, int):
        return 3
    if isinstance(v, Float):
        return 5
    if isinstance(v, float):
        return 6
    if isinstance(v, bytes):
        return 7
    if isinstance(v, str):
        return 8
    if isinstance(v, list):
        return 9
    if isinstance(v, dict):
        return 10
    raise TypeError(type(v))


def _name(s):
    raw = s.encode("utf-8")
    return struct.pack("<H", len(raw)) + raw


def disk_payload(v):
    t = tag_id(v)
    if t == 1:
        return struct.pack("<b", v)
    if t == 2:
        return struct.pack("<h", v)
    if t == 3:
        return struct.pack("<i", v)
    if t == 4:
        return struct.pack("<q", v)
    if t == 5:
        return struct.pack("<f", v)
    if t == 6:
        return struct.pack("<d", v)
    if t == 7:
        return struct.pack("<i", len(v)) + v
    if t == 8:
        return _name(v)
    if t == 9:
        return bytes([v.elem]) + struct.pack("<i", len(v)) + b"".join(disk_payload(x) for x in v)
    out = b""
    for k, x in v.items():
        out += bytes([tag_id(x)]) + _name(k) + disk_payload(x)
    return out + b"\x00"


def disk_root(compound):
    """NBT su disco di un compound radice senza nome."""
    return b"\x0a" + _name("") + disk_payload(compound)


def block_hash(name, states):
    """FNV-1a dell'NBT su disco `{name, states}` (stati in ordine alfabetico): l'ID di rete di uno stato di blocco."""
    nbt = disk_root({"name": name, "states": {k: states[k] for k in sorted(states)}})
    h = 0x811C9DC5
    for c in nbt:
        h = ((h ^ c) * 0x01000193) & 0xFFFFFFFF
    return h - (1 << 32) if h >= 1 << 31 else h
