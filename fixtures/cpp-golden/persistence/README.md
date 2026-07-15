# Historical C++ persistence fixtures

`scene.xml` and `loop.xml` are serialized by a small C++ libxml2 harness using
the same document construction, property order, numeric formatting, and
`xmlSaveFormatFile` path as `TriggerMap::GoSave` and
`LoopManager::SetupSaveLoop`. They exercise loop references, snapshots, XML
escaping, loop status/volume fields, and loop timing metadata without starting
audio or UI subsystems.

`fweelin.xml` remains the byte-for-byte historical application configuration.
`PROVENANCE` identifies the exact source locations and libxml2 version;
`MANIFEST.sha256` covers every file in this directory except itself.

