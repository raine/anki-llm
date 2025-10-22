import json
import requests

ANKI_CONNECT_URL = "http://127.0.0.1:8765"
DECK_NAME = "Glossika-ENJA [2001-3000]"

def anki_request(action, **params):
    """
    Helper function to send requests to AnkiConnect.
    """
    payload = {"action": action, "params": params, "version": 6}
    try:
        response = requests.post(ANKI_CONNECT_URL, data=json.dumps(payload))
        response.raise_for_status()
        response_json = response.json()
        if response_json.get("error"):
            raise Exception(f"AnkiConnect API error: {response_json['error']}")
        return response_json.get("result")
    except requests.exceptions.RequestException as e:
        raise Exception(f"Could not connect to AnkiConnect. Is Anki running? Error: {e}")

def inspect_fields():
    print("=" * 60)
    print("Inspecting Fields in Deck")
    print("=" * 60)

    try:
        # Find notes in the deck
        query = f'deck:"{DECK_NAME}"'
        note_ids = anki_request('findNotes', query=query)
        print(f"\n✓ Found {len(note_ids)} notes in '{DECK_NAME}'.")

        if not note_ids:
            print("No notes found.")
            return

        # Get info for the first note to see available fields
        notes_info = anki_request('notesInfo', notes=note_ids[:1])

        if notes_info:
            note = notes_info[0]
            print(f"\nNote ID: {note['noteId']}")
            print(f"Model: {note['modelName']}")
            print(f"\nAvailable fields:")
            for field_name, field_data in note['fields'].items():
                value = field_data['value'][:100]
                if len(field_data['value']) > 100:
                    value += "..."
                print(f"  - {field_name}: {value}")

    except Exception as e:
        print(f"\n✗ Error: {e}")

if __name__ == "__main__":
    inspect_fields()
