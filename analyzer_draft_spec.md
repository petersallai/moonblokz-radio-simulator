Simulator - Analyzer functionality
This specification describes a new functionality of the moonblokz-radio-simulator. The purpose of this functionality is to add the capability to visualize saved logs or live log streams in addition to the simulation.

In the code structure, the simulation module handles simulation functionality, while the ui module handles visualization for both the simulation and the analyzer. The analyzer module is dedicated to the new analyzer functionality. The analyzer communicates with the ui using the same queues as the simulation.

The application already has an opening screen with the three options: simulation, real-time tracking, and log visualization. 

In the real-time tracking option, there should be two buttons (vertically stacked). The first button's title should be "Select scene", and the second's should be "Connect to stream". Clicking the button opens a file chooser (as in the simulation now). If a scene file is selected, the file name and a checkmark should be shown on the button. If the second file is selected (the order is not important), the application should initialize the analyzer module and go to the main screen.

In log visualization column, also two buttons should be shown (work the same as real-time tracking) with the titles: "Select scene" and "Open log file). 

The first buttons in all three columns should have the same vertical position.

When the mode is selected, the application first loads the scene file. The scene format is the same for all modes. Except for the following: 
path-loss parameters, lora-parameters, and radio_module_config are only mandatory in simulation mode. 
A new parameter for all nodes: effective_distance is only mandatory in real-time tracking and log visualization modes. effective_distance is the node's effective radio transmitting distance in meters.

For this task, the scene file datatype, the loading logic, and validation should be refactored: move to a separate module (from simulation)

After the scene file is loaded, the application should open the log file. At this point, real-time tracking and log visualization are different:
In real-time tracking, after opening the file, the application should jump to the end of the file and start reading new lines (the file is appended by a separate process).
In log visualization, the application should read the file from the beginning, line by line. If the end-of-file is reached, the visualization ends (a pop-up message should be displayed on the UI).

All log lines start with a timestamp (in the following format: 2025-10-23T18:00:00Z). In both real-time tracking and log visualization modes, the application stores the timestamp of the first processed line as a reference. For the next line, it compares the reference+elapsed time since processing the reference line and the timestamp in the new line. If the timestamp in the new line is greater than the reference time + elapsed time, the program should wait until the reference time + elapsed time, since processing the reference line will make the timestamp equal to the new line's timestamp, and then process the line after the wait. If the timestamp in the new line is less, the program should process the line immediately and use its timestamp as a new reference time.

Architecturally, the analyzer module should also include an embassy async task that performs log file parsing, and it communicates with the UI through queues.

The log lines will have the following format and processing requirements:

1, Send packet:
Example log line format: 
moonblokz_radio_lib::radio_devices::rp_lora_sx1262: [3094] *TM1* Packet transmitted: type: 6, sequence: 30940779, length: 215, packet: 1/10

The *TM1* string is the identifier for send packet messages. The beginning of the line is not important, only the data fields and the *TM1* string.
The nodeid is in the [] brackets
The sequence and packet fields are optional.
What to do with the line:
A message should be sent to the UIRefreshQueue with NodeSentRadioMessage(nodeId, message type, effective distance for the node).
In an internal data structure, all incoming and outgoing packets for a node should be stored for serving RequestNodeInfo commands. 

2, Receive packet:
Example log line format: 
moonblokz_radio_lib::radio_devices::rp_lora_sx1262:[3094] *TM2* Packet received:  sender: 3093, type: 6, sequence: 30940779, length: 215, packet: 1/10, link quality: 26
The *TM2* string is the identifier for send packet messages. The beginning of the line is not important, only the data fields and the *TM2* string.
The nodeid is in the [] brackets
The sequence and packet fields are optional.
What to do with the line:
In an internal data structure, all incoming and outgoing packets for a node should be stored for serving RequestNodeInfo commands. 
3, Start measurement:
Example log line format: [3094] *TM3* Start measurement: sequence: 321312
The *TM3* string is the identifier for start measurement packet messages. The beginning of the line is not important, only the sequence field and the *TM3* string.
The nodeid is in the [] brackets

What to do with the line:
The measurement's sequence identifier should be stored internally for later use.
A message should be sent to the UIRefreshQueue with NodeReachedInMeasurement(nodeId, sequence).

4, Received full message:
Example log line format: [3094] *TM4* Routing message to incoming queue: sender: 3093, type: 6, length: 2000, sequence: 321312

What to do with the line:
If the sequence equals to the stored measurement identifier a message should be sent to the UIRefreshQueue with NodeReachedInMeasurement(nodeId, sequence).

On the UI, the sim time should show the timestamp of the last processed line in both modes. In real-time tracking mode, a new indicator should be shown in this category (next to sim time): Delay: The delay between the real clock and the timestamp of the last processed line (it is only updated when a new line is processed)

On the UI, the speed control should be hidden in real-time tracking mode. In log visualization, the time control should be here, but without the Auto speed checkbox.

The Start / Reset measurement button should be hidden in log visualization mode.
