#!/bin/sh
# Checks that the winpthreads functions `addon/src/absent_imports.rs` relies on being dead
# are still dead in the DLL this tree links.
#
# That file takes SuspendThread, GetThreadContext, SetThreadContext, ResumeThread and
# SetThreadPriority out of the import table by binding them to stand-ins that only fail. It is
# safe for one reason: the only code that calls them is pthread_cancel, pthread_create and
# pthread_setschedparam, and nothing reaches those (pthread_cancel's one caller, pthread_kill,
# is unreferenced too). If a toolchain upgrade or a C++ dependency ever starts using one of
# them, the link still succeeds and the import check of the README still prints nothing, but
# pthread_create would return success for a thread that never runs. This is the check that
# turns red instead.
#
# It links the addon once more, apart from the release build (own target directory, symbol
# table kept, linker map with --cref), and fails if:
#
#   1. the linker's cross-reference table names any object outside libpthread.a as referencing
#      pthread_create, pthread_cancel, pthread_kill or pthread_setschedparam, or anything but
#      the addon's own object as defining one of the five __imp_ stand-ins, or anything but
#      libpthread.a as referencing them;
#   2. any instruction of the linked DLL outside those four functions calls, jumps to or takes
#      the address of one of them, or goes through one of the five stand-ins;
#   3. the address of one of the four is stored anywhere in the DLL;
#   4. one of the five functions is back in the import table.
#
# Addresses are read from the DLL it has just linked; none is written here. It fails closed:
# no symbol table, no map or no disassembly is an error, not a pass.
#
# Usage: scripts/check-dead-thread-code.sh        (from anywhere; run it on every release build)
# Exit:  0 dead, 1 something references them, 2 the check could not be made.
#
# CHECK_DEAD_THREAD_CODE_LINK_ARGS adds linker arguments (space separated) to the check link.
# It exists to prove the check measures something: link an object that calls pthread_create
# and this must fail. The README shows how.
set -eu

target=x86_64-pc-windows-gnu
objdump=${OBJDUMP:-x86_64-w64-mingw32-objdump}
dead='pthread_create pthread_cancel pthread_kill pthread_setschedparam'
standins='SuspendThread GetThreadContext SetThreadContext ResumeThread SetThreadPriority'

cd "$(dirname "$0")/.."
root=$(pwd)
dir=$root/target/check-dead-thread-code
dll=$dir/$target/release/tyrian_companion_nexus.dll
# A new map name on every run: the map path is a compiler argument, so cargo links again even
# when no source changed. A toolchain upgrade changes no source, and it is what this is for.
map=$dir/link.$$.map
disassembly=$dir/dll.dis
contents=$dir/dll.hex

cannot() {
	echo "check-dead-thread-code: CANNOT CHECK: $*" >&2
	exit 2
}

command -v cargo >/dev/null 2>&1 || cannot "cargo not found"
command -v "$objdump" >/dev/null 2>&1 || cannot "$objdump not found (set OBJDUMP)"

mkdir -p "$dir"
rm -f "$dir"/link.*.map "$disassembly" "$contents"

set --
for argument in ${CHECK_DEAD_THREAD_CODE_LINK_ARGS-}; do
	set -- "$@" -C "link-arg=$argument"
done
cargo rustc --release --locked --target "$target" -p tyrian_companion_nexus --target-dir "$dir" -- \
	-C strip=debuginfo -C "link-arg=-Wl,-Map=$map,--cref" "$@" >&2 || cannot "the check link failed"

[ -s "$map" ] || cannot "the linker wrote no map at $map"
[ -s "$dll" ] || cannot "no DLL at $dll"
if "$objdump" -t "$dll" | grep -q '^no symbols'; then
	cannot "the check DLL has no symbol table"
fi
"$objdump" -d --no-show-raw-insn "$dll" >"$disassembly" || cannot "could not disassemble $dll"
"$objdump" -s "$dll" >"$contents" || cannot "could not dump $dll"
[ -s "$disassembly" ] && [ -s "$contents" ] || cannot "objdump wrote nothing for $dll"

status=0

# Runs one awk pass, prints its lines sorted, and folds its verdict into `status`: an awk exit
# of 2 means that pass could not check, anything else but 0 that it found a reference.
pass() {
	result=0
	lines=$(awk -v dead="$dead" -v standins="$standins" "$@") || result=$?
	[ -z "$lines" ] || printf '%s\n' "$lines" | sort
	[ "$result" -ne 2 ] || cannot "see the line above"
	[ "$result" -eq 0 ] || status=1
}

# 1. Who references what, object by object, as the linker saw it.
pass '
	BEGIN {
		count = split(dead, names, " ")
		for (i = 1; i <= count; i++) is_dead[names[i]] = 1
		count = split(standins, names, " ")
		for (i = 1; i <= count; i++) is_standin["__imp_" names[i]] = 1
	}
	/^Cross Reference Table/ { table = 1; next }
	!table || /^$/ { next }
	/^[^ \t]/ {
		symbol = $1
		file = $0
		sub(/^[^ \t]+[ \t]+/, "", file)
		first = 1
	}
	/^[ \t]/ {
		file = $0
		sub(/^[ \t]+/, "", file)
		first = 0
	}
	file == "" || file == "File" { next }
	{
		pthread = file ~ "lib(win)?pthread[.]a[(]"
		own = file ~ "/tyrian_companion_nexus[^/]*[.]o$"
		if (symbol in is_dead) {
			seen[symbol] = 1
			if (!pthread) {
				printf "  FAIL %s is referenced by %s\n", symbol, file
				bad = failed[symbol] = 1
			}
		}
		if (symbol in is_standin) {
			seen[symbol] = 1
			if (first && !own) {
				printf "  FAIL %s is defined by %s, not by the addon\n", symbol, file
				bad = failed[symbol] = 1
			}
			if (!first && !pthread) {
				printf "  FAIL %s is referenced by %s\n", symbol, file
				bad = failed[symbol] = 1
			}
		}
	}
	END {
		if (!table) {
			print "  FAIL the map has no cross-reference table"
			exit 2
		}
		for (symbol in is_dead) if (!(symbol in failed))
			printf "  %-26s %s\n", symbol, ((symbol in seen) ? "no object outside libpthread.a references it" : "not in the link")
		for (symbol in is_standin) if (!(symbol in failed))
			printf "  %-26s %s\n", symbol, ((symbol in seen) ? "defined by the addon, referenced only by libpthread.a" : "not in the link")
		exit bad
	}
' "$map"

# 2. Every instruction of the DLL: nothing outside the four functions may mention them or the
#    stand-ins. objdump names the target of each call, jump and rip-relative address.
pass '
	BEGIN {
		count = split(dead, names, " ")
		for (i = 1; i <= count; i++) is_dead[names[i]] = 1
		count = split(standins, names, " ")
		for (i = 1; i <= count; i++) is_standin["__imp_" names[i]] = 1
	}
	/^[0-9a-f]+ <.*>:$/ {
		current = $2
		sub(/^</, "", current)
		sub(/>:$/, "", current)
		labels++
		next
	}
	/<[^>]+>/ {
		line = $0
		while (match(line, /<[^>]+>/)) {
			name = substr(line, RSTART + 1, RLENGTH - 2)
			line = substr(line, RSTART + RLENGTH)
			sub(/\+0x[0-9a-f]+$/, "", name)
			if ((name in is_dead) && !(current in is_dead)) {
				printf "  FAIL %s reaches %s:%s\n", current, name, $0
				bad = 1
			}
			if (name in is_standin) {
				if (current in is_dead) {
					if (index(users[name] " ", " " current " ") == 0) users[name] = users[name] " " current
				} else {
					printf "  FAIL %s goes through the stand-in %s:%s\n", current, name, $0
					bad = failed[name] = 1
				}
			}
		}
	}
	END {
		if (labels < 100) {
			print "  FAIL the disassembly has no function names"
			exit 2
		}
		for (name in is_standin) if (!(name in failed))
			printf "  %-26s used only by:%s\n", name, (users[name] == "" ? " nobody" : users[name])
		exit bad
	}
' "$disassembly"

# 3. No stored pointer to any of the four, at any byte offset of any section.
pass '
	BEGIN {
		count = split(dead, names, " ")
		for (i = 1; i <= count; i++) is_dead[names[i]] = 1
	}
	FNR == NR {
		if ($0 ~ /^[0-9a-f]+ <.*>:$/) {
			name = $2
			sub(/^</, "", name)
			sub(/>:$/, "", name)
			if (name in is_dead) {
				address = $1
				while (length(address) < 16) address = "0" address
				bytes = ""
				for (i = 15; i >= 1; i -= 2) bytes = bytes substr(address, i, 2)
				pattern[name] = bytes
				found++
			}
		}
		next
	}
	/^Contents of section/ {
		tail = ""
		# Debug information lists the address of every function; it is not a reference. The
		# check link drops it, and a section of that kind is skipped if one is left.
		skip = ($4 ~ /^[.]debug/ || $4 ~ /^[\/]/)
		next
	}
	skip { next }
	/^ [0-9a-f]+ / {
		# The hex columns: 35 characters after the address, whatever its width.
		data = substr($0, index(substr($0, 2), " ") + 2, 35)
		gsub(/ /, "", data)
		window = tail data
		for (name in pattern) {
			at = index(window, pattern[name])
			if (at > 0 && at % 2 == 1) {
				printf "  FAIL a pointer to %s is stored near %s\n", name, $1
				bad = 1
			}
		}
		tail = substr(window, length(window) - 13)
	}
	END {
		printf "  %d of the four are in the DLL; pointers to them stored in it: %s\n", found, (bad ? "SOME" : "none")
		exit bad
	}
' "$disassembly" "$contents"

# 4. And the imports themselves, on the DLL of this same link.
imported=$("$objdump" -p "$dll" | grep -E "[[:space:]]($(echo "$standins" | tr ' ' '|'))\$" || true)
if [ -n "$imported" ]; then
	echo "  FAIL imported again:"
	echo "$imported"
	status=1
else
	echo "  import table: none of the five"
fi

rm -f "$map" "$disassembly" "$contents"
if [ "$status" -ne 0 ]; then
	echo "check-dead-thread-code: FAIL, see addon/src/absent_imports.rs before shipping this DLL"
	exit 1
fi
echo "check-dead-thread-code: OK"
