# Copied for ns-next from Are We Fast Yet
# (https://github.com/smarr/are-we-fast-yet, commit
# 74306fec151070fd07157cefeacf19e7e0bcdc89, benchmarks/Python/som/identity_dictionary.py),
# unchanged.

from som.dictionary import Entry, Dictionary


class _IdEntry(Entry):
    def match(self, hash_, key):
        return self.hash == hash_ and self.key is key


class IdentityDictionary(Dictionary):
    def _new_entry(self, key, value, hash_):
        return _IdEntry(hash_, key, value, None)
