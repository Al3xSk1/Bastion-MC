"""A minimal class-file reader: access flags, names, supertypes, fields and methods."""
import struct

ACC_PUBLIC, ACC_PRIVATE, ACC_PROTECTED, ACC_STATIC, ACC_FINAL = 0x1, 0x2, 0x4, 0x8, 0x10
ACC_INTERFACE, ACC_ABSTRACT, ACC_SYNTHETIC, ACC_ENUM, ACC_BRIDGE, ACC_VARARGS = 0x200, 0x400, 0x1000, 0x4000, 0x40, 0x80

class ClassInfo:
    __slots__ = ('name', 'access', 'super', 'interfaces', 'fields', 'methods')

def parse(data):
    if data[:4] != b'\xca\xfe\xba\xbe':
        raise ValueError('not a class file')
    count = struct.unpack('>H', data[8:10])[0]
    pool = [None] * count
    i, at = 1, 10
    while i < count:
        tag = data[at]
        if tag == 1:
            length = struct.unpack('>H', data[at + 1:at + 3])[0]
            pool[i] = ('utf8', data[at + 3:at + 3 + length].decode('utf-8', 'surrogatepass'))
            at += 3 + length
        elif tag in (3, 4):
            at += 5
        elif tag in (5, 6):
            at += 9
            i += 1
        elif tag == 7:
            pool[i] = ('class', struct.unpack('>H', data[at + 1:at + 3])[0])
            at += 3
        elif tag in (8, 16, 19, 20):
            at += 3
        elif tag in (9, 10, 11, 12, 17, 18):
            at += 5
        elif tag == 15:
            at += 4
        else:
            raise ValueError('tag %d' % tag)
        i += 1
    def utf8(index):
        return pool[index][1]
    def cls(index):
        return utf8(pool[index][1]) if index else None
    info = ClassInfo()
    info.access, this, sup = struct.unpack('>HHH', data[at:at + 6])
    info.name, info.super = cls(this), cls(sup)
    at += 6
    n = struct.unpack('>H', data[at:at + 2])[0]
    at += 2
    info.interfaces = [cls(struct.unpack('>H', data[at + 2 * k:at + 2 * k + 2])[0]) for k in range(n)]
    at += 2 * n
    def members():
        nonlocal at
        out = []
        n = struct.unpack('>H', data[at:at + 2])[0]
        at += 2
        for _ in range(n):
            access, name, desc, attrs = struct.unpack('>HHHH', data[at:at + 8])
            at += 8
            for _ in range(attrs):
                length = struct.unpack('>I', data[at + 2:at + 6])[0]
                at += 6 + length
            out.append((access, utf8(name), utf8(desc)))
        return out
    info.fields = members()
    info.methods = members()
    return info
