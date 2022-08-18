from .fwd_fixedbytes import FwdFixedBytes
from .fwd_intsequence import FwdIntSequence
from .fwd_intstored import FwdIntStored
from .fwd_uuid import FwdUuid
from .inv_hash import InvHash
from .inv_intsequence import InvIntSequence
from .inv_intstored import InvIntStored

fwd = {c.NAME: c for c in [FwdFixedBytes, FwdIntSequence, FwdIntStored, FwdUuid]}
inv = {c.NAME: c for c in [InvHash, InvIntSequence, InvIntStored]}
