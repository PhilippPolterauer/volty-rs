#!/usr/bin/env -S uv run
# /// script
# dependencies = [
#     "winsdk",
# ]
# ///

import asyncio
from winsdk.windows.devices.enumeration import DeviceInformation


BATTERY_KEYS = [
    "System.Devices.Aep.Bluetooth.BatteryLevel",
    "System.Devices.BatteryLifePercent",
]


async def main():
    print("Querying Windows Bluetooth devices...\n")

    devices = await DeviceInformation.find_all_async()

    found = False

    for device in devices:
        props = device.properties

        battery = None

        for key in BATTERY_KEYS:
            if key in props:
                battery = props[key]
                break

        if battery is not None:
            found = True

            print("=" * 60)
            print("Name:", device.name)
            print("Id  :", device.id)
            print("Battery:", battery, "%")
            print()

    if not found:
        print("No devices exposing standard battery properties found.")
        print()
        print("Dumping interesting Bluetooth devices:")
        print()

        for device in devices:
            if (
                "bluetooth" in device.id.lower()
                or "bth" in device.id.lower()
            ):
                print("=" * 60)
                print("Name:", device.name)
                print("Id:", device.id)

                for key, value in device.properties.items():
                    if (
                        "battery" in str(key).lower()
                        or "bluetooth" in str(key).lower()
                        or "aep" in str(key).lower()
                    ):
                        print(f"  {key} = {value}")

                print()


if __name__ == "__main__":
    asyncio.run(main())