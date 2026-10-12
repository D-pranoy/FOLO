"""
The main module which spawns a gui and a effect thread and opens a socket
connection to the specified host. The effect algorithm is in the effect
module. Change this file to develop your effect. Change the host and port to
your needs in this main file and also the values representing your led
configuration.

Created on 27.11.2014
Last modified on 13.3.2016

@author: Fabian Hertwig
@author: Juha Rantanen
"""

import os
import sys
from threading import Thread
import runpy
import argparse
import time
import json
import operator
import commentjson

BASE_DIR = os.path.dirname(os.path.abspath(__file__))
os.chdir(BASE_DIR)
sys.path.insert(0, BASE_DIR)

from app import hyperion
from app.json_client import fetch_server_info

# Change this according to your led configuration.
# horizontal_led_num = 14
# vertical_led_num = 8
# first_led_offset_num = 0
# leds_in_clockwise_direction = True
# has_corner_leds = False

def create_parser(config):
    """
    Creates ArgumentParser using config values as defaults.
    """
    parser = argparse.ArgumentParser()
    parser.add_argument("--gui",
        help="enable GUI",
        action="store_true")
    parser.add_argument("--idx",
        help="show led indexes in gui",
        action="store_true")
    parser.add_argument("--json",
        help="enable JSON client",
        action="store_true",
        default=config['json'])
    parser.add_argument("--proto",
        help="enable protobuf client (disables JSON)",
        default=config['proto'])
    parser.add_argument("--effect",
        help="select effect",
        default=config['effect'])
    parser.add_argument("--config",
        help="path to a hyperion config file with the led layout "
             "(default: read the layout from the running hyperion server)",
        default=config['config'])
    parser.add_argument("--layout-port",
        help="hyperion JSON port used to read the led layout (default 19444)",
        type=int,
        default=config.get('layout-port', 19444))
    parser.add_argument("--priority",
        help="hyperion priority channel (default 100, lower wins)",
        type=int,
        default=config.get('priority', 100))
    parser.add_argument("--list-devices",
        help="list audio sources and exit",
        action="store_true")
    parser.add_argument("--host",
        help="JSON host (default localhost)",
        default=config['host'])
    parser.add_argument("--port",
        help="JSON port (default 19444)",
        type=int,
        default=config['port'])
    parser.add_argument("--audiosrc",
        help="audio source: autoaudiosrc (what you hear on the default "
             "output) or part of a device name, see --list-devices",
        default=config['audiosrc'])
    parser.add_argument("--interval",
        help="Interval for sending data (ms)",
        type=int,
        default=config['interval'])
    parser.add_argument("--matrix",
        help="use matrix instead of TV setup",
        action="store_true",
        default=config['matrix'])
    return parser

def run_effect(effect='effect'):
    """
    Runs the module effect. Copy any hyperion effect code in this module or
    create your own. Note that effects that call hyperion.setImage(img) are
    not supported by the local gui, use hypersim instead.
    """
    runpy.run_path(os.path.join(BASE_DIR, "effects", effect + '.py'))

def read_config(file_path):

    """
    Parses config file.
    """
    with open(file_path) as config_json:
        return commentjson.load(config_json)

def led_bounds(led):
    """
    Returns (hmin, hmax, vmin, vmax) for classic (hscan/vscan) and
    Hyperion.NG (hmin/hmax/vmin/vmax) led definitions.
    """
    if 'hscan' in led:
        return (led['hscan']['minimum'], led['hscan']['maximum'],
                led['vscan']['minimum'], led['vscan']['maximum'])
    return (led['hmin'], led['hmax'], led['vmin'], led['vmax'])

def read_hyperion_config(file_path=None, host='localhost', port=19444):

    """
    Reads the led layout from a hyperion config file, or from the running
    hyperion server when no file is given.
    """
    if file_path:
        with open(file_path) as hyperion_config_json:
            config = commentjson.load(hyperion_config_json)
        config_leds = config.get('leds', [])
    else:
        config_leds = fetch_server_info(host, port)['leds']
    return parse_layout(config_leds)

def parse_layout(config_leds):
    """
    Splits the leds to top, right, bottom and left lists.
    """
    leds = []

    x_coords = []
    y_coords = []

    for led in config_leds:
        hmin, hmax, vmin, vmax = led_bounds(led)
        h_center = round(((hmin + hmax) / 2) * 100, 2)
        v_center = round(((vmin + vmax) / 2) * 100, 2)
        x_coords.append(h_center)
        y_coords.append(v_center)
        leds.append({'x': h_center, 'y': v_center})


    if not leds:
        raise RuntimeError('No leds found in the hyperion layout')

    if len(set(x_coords)) == 1 or len(set(y_coords)) == 1:
        # Single strip (all leds on one row or column): use the first half
        # as the "left" side and the second half as the "right" side
        half = len(leds) // 2
        print('Strip layout detected ({} leds)'.format(len(leds)))
        return (leds, [], list(range(half, len(leds))), [], list(range(0, half)))

    xcounts = []
    left = None
    right = None


    for x in set(x_coords):
        xcounts.append({'x': x, 'count': x_coords.count(x)})

    if len(dict((xcount['count'], xcount) for xcount in xcounts).values()) > 1:
        # Position might not be minimum for TV setups
        xcounts.sort(key=operator.itemgetter('count'))
        right = xcounts[len(xcounts)-2]
        left = xcounts[len(xcounts)-1]
    else:
        # Position should be minimum for matrix setups
        xcounts.sort(key=operator.itemgetter('x'))
        right = xcounts[len(xcounts)-1]
        left = xcounts[0]


    if right['x'] < left['x']:
        left, right = right, left


    ycounts = []
    top = None
    bottom = None

    for y in set(y_coords):
        ycounts.append({'y': y, 'count': y_coords.count(y)})

    if len(dict((ycount['count'], ycount) for ycount in ycounts).values()) > 1:
        # Position might not be minimum for TV setups
        ycounts.sort(key=operator.itemgetter('count'))
        bottom = ycounts[len(ycounts)-2]
        top = ycounts[len(ycounts)-1]
    else:
        # Position should be minimum for matrix setups
        ycounts.sort(key=operator.itemgetter('y'))
        bottom = ycounts[len(ycounts)-1]
        top = ycounts[0]


    if bottom['y'] < top['y']:
        top, bottom = bottom, top

    leds_left = []
    leds_right = []
    leds_top = []
    leds_bottom = []

    for i, led in enumerate(leds):
        x = led['x']
        y = led['y']
        if x == left['x']:
            leds_left.append(i)
        elif x == right['x']:
            leds_right.append(i)
        elif y == top['y']:
            leds_top.append(i)
        elif y == bottom['y']:
            leds_bottom.append(i)

    # Sort the lists
    leds_top.sort(key=lambda i: leds[i]['x'], reverse=False)
    leds_right.sort(key=lambda i: leds[i]['y'], reverse=False)
    leds_bottom.sort(key=lambda i: leds[i]['x'], reverse=True)
    leds_left.sort(key=lambda i: leds[i]['y'], reverse=True)

    # Not the lists run like this:

    #  >>>>>>> TOP >>>>>>>
    #  ^                 v
    #  ^                 v
    # LEFT              RIGHT
    #  ^                 v
    #  ^                 v
    #  <<<<< BOTTOM <<<<<<

    # print 'leds_top: {}'.format(leds_top)
    # print 'leds_right: {}'.format(leds_right)
    # print 'leds_bottom: {}'.format(leds_bottom)
    # print 'leds_left: {}'.format(leds_left)

    return (leds, leds_top, leds_right, leds_bottom, leds_left)

def run_json(host, port, interval, priority):
    from app.json_client import JsonClient
    sleep_time = interval / 1000.0
    json_client = JsonClient(host, port, priority=priority)
    json_client.connect()
    while not hyperion.abort():
        json_client.send_led_data(hyperion.get_led_data())
        time.sleep(sleep_time)
    json_client.disconnect()

def run_proto(host, port, interval, priority):
    os.environ.setdefault('PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION', 'python')
    from lib.hyperion.Hyperion import Hyperion
    proto_client = Hyperion(host, port)
    sleep_time = interval / 1000.0
    while not hyperion.abort():
        w, h, c = hyperion.get_image_data()
        # print "Sending proto: {}, {}, {}\n".format(w, h, len(c))
        # if w < 1 or h < 1 or len(c) is not w*h*3:
        #     continue
        proto_client.sendImage(w, h, c, priority)
        time.sleep(sleep_time)
    proto_client.clear(priority)

def main():

    args = create_parser(read_config(os.path.join(BASE_DIR, 'config.json'))).parse_args()

    if args.list_devices:
        from effects.spectrum_dump import list_devices
        list_devices()
        return

    if isinstance(args.proto, str):
        args.proto = args.proto.lower() in ('1', 'true', 'yes')

    leds, leds_top, leds_right, leds_bottom, leds_left = read_hyperion_config(
        args.config, args.host, args.layout_port)
    print('Led layout: {} leds'.format(len(leds)))

    hyperion.init(leds, leds_top, leds_right, leds_bottom, leds_left)

    json_thread = None
    proto_thread = None

    if args.json and not args.proto:
        json_thread = Thread(
            target=run_json, kwargs={
                'host': args.host,
                'port': args.port,
                'interval': args.interval,
                'priority': args.priority
            }
        )
        json_thread.start()
    elif args.proto:
        proto_thread = Thread(target=run_proto, kwargs={
            'host': args.host,
            'port': args.port,
            'interval': args.interval,
            'priority': args.priority
            }
        )
        proto_thread.start()

    with open(os.path.join(BASE_DIR, 'effects', args.effect + '.json')) as effect_json:
        effect = json.load(effect_json)
        effect_args = effect.get('args', {})
        effect_args['audiosrc'] = args.audiosrc
        effect_args['matrix'] = args.matrix
        hyperion.set_args(effect_args)

    # create own thread for the effect
    effect_thread = Thread(target=run_effect, kwargs={'effect': args.effect})

    effect_thread.start()

    if args.gui:
        from app import gui
        gui.createWindow(args.idx)

        # After the window was closed abort the effect through the fake hyperion module
        hyperion.set_abort(True)
    else:
        print("Exit by typing 'x' or 'exit' (or Ctrl+C)")
        cmd = ''
        try:
            while cmd != 'x' and cmd != 'exit':
                cmd = input().strip()
        except (KeyboardInterrupt, EOFError):
            pass

        hyperion.set_abort(True)

    # wait for the thread to stop
    effect_thread.join()

    if json_thread is not None:
        json_thread.join()

    if proto_thread is not None:
        proto_thread.join()

    print("Exiting")


if __name__ == '__main__':
    main()
