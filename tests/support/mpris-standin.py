#!/usr/bin/python3 -I
"""A stand-in for the Spotify desktop app's MPRIS interface, for tests/spotify.rs.

It owns a bus name (default org.mpris.MediaPlayer2.spotify) on the bus named by
$BASTION_TEST_BUS - a private dbus-daemon started by the test - and serves
/org/mpris/MediaPlayer2 with org.mpris.MediaPlayer2 and org.mpris.MediaPlayer2.Player the way
Spotify does: Metadata with mpris:trackid (an object path), mpris:length (uint64), mpris:artUrl,
xesam:title/album/artist/url; PlaybackStatus, LoopStatus, Shuffle, Volume, Position.

It is written with GDBus (python3-gobject), so the launcher's own D-Bus code is tested against
an independent implementation of the protocol.

It refuses to run on anything that looks like a desktop session bus: it never touches the
user's session bus, and it is never the real Spotify app.

Prints one line per thing it is asked to do ("call Play", "set Volume 0.4",
"call OpenUri spotify:track:...") and "ready" once it owns its name. Reads standard input:
"metadata <n>" switches to track n, "quit" (or end of input) ends it.
"""
import os, sys, threading

import gi
gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib

OBJECT = "/org/mpris/MediaPlayer2"
ROOT = "org.mpris.MediaPlayer2"
PLAYER = "org.mpris.MediaPlayer2.Player"

XML = """<node>
<interface name="org.mpris.MediaPlayer2">
  <method name="Raise"/><method name="Quit"/>
  <property name="Identity" type="s" access="read"/>
  <property name="CanQuit" type="b" access="read"/>
  <property name="CanRaise" type="b" access="read"/>
  <property name="HasTrackList" type="b" access="read"/>
  <property name="SupportedUriSchemes" type="as" access="read"/>
  <property name="SupportedMimeTypes" type="as" access="read"/>
</interface>
<interface name="org.mpris.MediaPlayer2.Player">
  <method name="Next"/><method name="Previous"/><method name="Pause"/>
  <method name="PlayPause"/><method name="Stop"/><method name="Play"/>
  <method name="Seek"><arg direction="in" name="Offset" type="x"/></method>
  <method name="SetPosition"><arg direction="in" name="TrackId" type="o"/><arg direction="in" name="Position" type="x"/></method>
  <method name="OpenUri"><arg direction="in" name="Uri" type="s"/></method>
  <signal name="Seeked"><arg name="Position" type="x"/></signal>
  <property name="PlaybackStatus" type="s" access="read"/>
  <property name="LoopStatus" type="s" access="readwrite"/>
  <property name="Rate" type="d" access="readwrite"/>
  <property name="Shuffle" type="b" access="readwrite"/>
  <property name="Metadata" type="a{sv}" access="read"/>
  <property name="Volume" type="d" access="readwrite"/>
  <property name="Position" type="x" access="read"/>
  <property name="MinimumRate" type="d" access="read"/>
  <property name="MaximumRate" type="d" access="read"/>
  <property name="CanGoNext" type="b" access="read"/>
  <property name="CanGoPrevious" type="b" access="read"/>
  <property name="CanPlay" type="b" access="read"/>
  <property name="CanPause" type="b" access="read"/>
  <property name="CanSeek" type="b" access="read"/>
  <property name="CanControl" type="b" access="read"/>
</interface>
</node>"""

TRACKS = [
    ("4uLU6hMCjMI75M1A2tKUQC", "Harder, Better, Faster, Stronger", "Discovery", ["Daft Punk"], 224000000,
     "ab67616d0000b2734ce8b4e42588bf18182a1ad2"),
    ("0DiWol3AO6WpXZgp0goxAV", "One More Time", "Discovery", ["Daft Punk"], 320000000,
     "ab67616d0000b2734ce8b4e42588bf18182a1ad3"),
    ("7ouMYWpwJ422jRcDASZB7P", "Knights of Cydonia", "Black Holes and Revelations", ["Muse"], 366000000,
     "ab67616d0000b27328933b808bfb4cbbd0385400"),
]


def say(line):
    sys.stdout.write(line + "\n")
    sys.stdout.flush()


class Player:
    def __init__(self, connection):
        self.connection = connection
        self.track = 0
        self.status = "Paused"
        self.loop = "None"
        self.shuffle = False
        self.volume = 0.5
        self.position = 0

    def metadata(self):
        tid, title, album, artists, length, art = TRACKS[self.track]
        return {
            "mpris:trackid": GLib.Variant("o", "/com/spotify/track/" + tid),
            "mpris:length": GLib.Variant("t", length),
            "mpris:artUrl": GLib.Variant("s", "https://i.scdn.co/image/" + art),
            "xesam:title": GLib.Variant("s", title),
            "xesam:album": GLib.Variant("s", album),
            "xesam:artist": GLib.Variant("as", artists),
            "xesam:albumArtist": GLib.Variant("as", artists),
            "xesam:url": GLib.Variant("s", "https://open.spotify.com/track/" + tid),
            "xesam:autoRating": GLib.Variant("d", 0.5),
            "xesam:discNumber": GLib.Variant("i", 1),
            "xesam:trackNumber": GLib.Variant("i", 4),
        }

    def get(self, interface, name):
        if interface == ROOT:
            return {
                "Identity": GLib.Variant("s", "Spotify"),
                "CanQuit": GLib.Variant("b", True),
                "CanRaise": GLib.Variant("b", False),
                "HasTrackList": GLib.Variant("b", False),
                "SupportedUriSchemes": GLib.Variant("as", ["spotify"]),
                "SupportedMimeTypes": GLib.Variant("as", []),
            }.get(name)
        return {
            "PlaybackStatus": GLib.Variant("s", self.status),
            "LoopStatus": GLib.Variant("s", self.loop),
            "Rate": GLib.Variant("d", 1.0),
            "Shuffle": GLib.Variant("b", self.shuffle),
            "Metadata": GLib.Variant("a{sv}", self.metadata()),
            "Volume": GLib.Variant("d", self.volume),
            "Position": GLib.Variant("x", self.position),
            "MinimumRate": GLib.Variant("d", 1.0),
            "MaximumRate": GLib.Variant("d", 1.0),
            "CanGoNext": GLib.Variant("b", True),
            "CanGoPrevious": GLib.Variant("b", True),
            "CanPlay": GLib.Variant("b", True),
            "CanPause": GLib.Variant("b", True),
            "CanSeek": GLib.Variant("b", True),
            "CanControl": GLib.Variant("b", True),
        }.get(name)

    def changed(self, *names):
        props = {name: self.get(PLAYER, name) for name in names}
        self.connection.emit_signal(None, OBJECT, "org.freedesktop.DBus.Properties", "PropertiesChanged",
                                    GLib.Variant("(sa{sv}as)", (PLAYER, props, [])))

    def call(self, connection, sender, path, interface, method, params, invocation):
        args = params.unpack()
        say("call " + " ".join([method] + [str(a) for a in args]))
        if interface == ROOT:
            invocation.return_dbus_error("org.mpris.MediaPlayer2.Error.NotSupported", "not here")
            return
        if method == "Play":
            self.status = "Playing"
            self.changed("PlaybackStatus")
        elif method == "Pause":
            self.status = "Paused"
            self.changed("PlaybackStatus")
        elif method == "PlayPause":
            self.status = "Paused" if self.status == "Playing" else "Playing"
            self.changed("PlaybackStatus")
        elif method in ("Next", "Previous"):
            self.track = (self.track + (1 if method == "Next" else -1)) % len(TRACKS)
            self.position = 0
            self.changed("Metadata")
        elif method == "SetPosition":
            self.position = args[1]
            self.connection.emit_signal(None, OBJECT, PLAYER, "Seeked", GLib.Variant("(x)", (self.position,)))
        elif method == "Seek":
            self.position = max(0, self.position + args[0])
            self.connection.emit_signal(None, OBJECT, PLAYER, "Seeked", GLib.Variant("(x)", (self.position,)))
        elif method == "OpenUri":
            self.track = 2
            self.status = "Playing"
            self.changed("Metadata", "PlaybackStatus")
        invocation.return_value(None)

    def get_property(self, connection, sender, path, interface, name):
        return self.get(interface, name)

    def set_property(self, connection, sender, path, interface, name, value):
        say("set %s %s" % (name, value.unpack()))
        if name == "Volume":
            self.volume = value.unpack()
        elif name == "Shuffle":
            self.shuffle = value.unpack()
        elif name == "LoopStatus":
            self.loop = value.unpack()
        else:
            return False
        self.changed(name)
        return True


def main():
    address = os.environ.get("BASTION_TEST_BUS", "")
    uid = os.getuid()
    if not address.startswith("unix:path=") or address in (
        "unix:path=/run/user/%d/bus" % uid, os.environ.get("DBUS_SESSION_BUS_ADDRESS", "-")
    ):
        sys.exit("mpris-standin: refusing to run on %r: only on a private test bus" % address)
    name = sys.argv[1] if len(sys.argv) > 1 else "org.mpris.MediaPlayer2.spotify"
    flags = Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION
    connection = Gio.DBusConnection.new_for_address_sync(address, flags, None, None)
    node = Gio.DBusNodeInfo.new_for_xml(XML)
    player = Player(connection)
    for interface in node.interfaces:
        connection.register_object(OBJECT, interface, player.call, player.get_property, player.set_property)
    loop = GLib.MainLoop()

    def acquired(conn, owned):
        say("ready")

    def lost(conn, lost_name):
        say("lost " + lost_name)
        loop.quit()

    Gio.bus_own_name_on_connection(connection, name, Gio.BusNameOwnerFlags.NONE, acquired, lost)

    def commands():
        for line in sys.stdin:
            words = line.split()
            if words[:1] == ["quit"]:
                break
            if words[:1] == ["metadata"] and len(words) == 2:
                def switch(n=int(words[1])):
                    player.track = n % len(TRACKS)
                    player.changed("Metadata")
                GLib.idle_add(switch)
        GLib.idle_add(loop.quit)

    threading.Thread(target=commands, daemon=True).start()
    loop.run()


if __name__ == "__main__":
    main()
