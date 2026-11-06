from pathlib import Path
import npids
from .utils import FileManager
from . import codecs


class FwdLookupBuilder:
    def __init__(self, path, min_block=None):
        self.path = Path(path)
        self.formats = {k: v() for k, v in npids.codecs.fwd.items()}
        self.seeded_formats = self.formats.copy()
        self.format = None
        self.seeds = []
        self.fout = FileManager(self.path, 'a')
        self.count = 0
        self.min_block = min_block or 2**8

    def add(self, id):
        if self.format is not None:
            enc_id = self.format.encode(id)
            if enc_id is None:
                self.format.reset()
                self.format = None
                self.seeded_formats = self.formats.copy()
                self.seeds = []
            else:
                self.count += 1
                self.fout.write(enc_id)

        if self.format is None:
            for f in list(self.seeded_formats):
                if not self.seeded_formats[f].seed(id):
                    self.seeded_formats[f].reset()
                    del self.seeded_formats[f]
            self.seeds.append(id)
            if len(self.seeds) >= self.min_block:
                self._commit()

    def close(self):
        self._commit()
        self.fout.close()

    def _commit(self):
        if self.count != 0:
            self.fout.write_doc_count(self.count)

        if self.seeds:
            self.format = min((f for f in self.seeded_formats.values()), key=lambda f: f.size())
            config = {'format': self.format.NAME}
            config.update(self.format.config())
            self.fout.write_header(0, len(self.seeds), config)
            self.count = 0
            for seed in self.seeds:
                self.fout.write(self.format.encode(seed))
                self.count += 1
            self.seeds = None

        self.fout.flush()


class InvLookupBuilder:
    @staticmethod
    def build(path):
        with npids.Lookup(path) as lookup, FileManager(path, 'a') as writer:
            if len(lookup.fwd.codecs) == 1 and isinstance(lookup.fwd.codecs[0].fmt, codecs.fwd['intsequence']):
                codecs.inv['intsequence'].build(lookup.fwd, writer)
            elif codecs.inv['intstored'].condition(lookup.fwd):
                codecs.inv['intstored'].build(lookup.fwd, writer)
            elif codecs.inv['intsequencemulti'].condition(lookup.fwd):
                codecs.inv['intsequencemulti'].build(lookup.fwd, writer)
            else:
                codecs.inv['hash'].build(lookup.fwd, writer)

    @staticmethod
    def _matches_intstored_case(lookup):
        pass
