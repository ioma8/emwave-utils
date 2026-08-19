import binascii

raw = "0600ff09ffa1010901a10185750975150026ff007508953e8100c00901a10185700970150026ff007508953e8100c00901a10185660966150026ff007508953e8100c00901a10185650965150026ff007508953e8100c00901a10185460946150026ff007508953e8100c00901a10185530953150026ff007508953e9100c00901a10185770977150026ff007508953fb100c00901a10185740974150026ff0075089508b100c00901a10185720972150026ff0075089501b100c00901a10185310931150026ff007508950cb100c00901a10185620962150026ff0075089508b100c00901a10185580958150026ff0075089501b100c00901a10185500950150026ff0075089501b100c00901a10185450945150026ff0075089505b100c00901a10185040904150026ff007508953eb100c00901a10185490949150026ff0075089525b100c0c0"
b = bytes.fromhex(raw)

print("Total descriptor size: %d bytes" % len(b))

# Decode report structure
reports = []
# Parse simple patterns: 0x85 (Report ID), 0x09 (Usage), 0x95 (Report Count), 0x81/91/b1 (Input/Output/Feature)
i = 0
cur_id = None
cur_usage = None
cur_count = None

while i < len(b):
    cmd = b[i]
    if cmd == 0x85:  # Report ID
        cur_id = b[i+1]
        i += 2
    elif cmd == 0x09:  # Usage
        cur_usage = b[i+1]
        i += 2
    elif cmd == 0x95:  # Report Count
        cur_count = b[i+1]
        i += 2
    elif cmd in (0x81, 0x91, 0xb1):  # Input, Output, Feature
        rtype = {0x81: "Input", 0x91: "Output", 0xb1: "Feature"}[cmd]
        char = chr(cur_id) if 32 <= cur_id <= 126 else "?"
        reports.append((cur_id, char, rtype, cur_count, cur_usage))
        i += 2
    else:
        i += 1
print("\nParsed Report Layout:")
print("%-10s %-6s %-10s %-12s %-10s" % ("Report ID", "Char", "Type", "Payload Size", "Usage"))
print("-" * 55)
for rid, char, rtype, count, usage in reports:
    print("0x%02x       '%s'    %-10s %-12d 0x%02x" % (rid, char, rtype, count, usage))
