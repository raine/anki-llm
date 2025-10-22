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

def main():
    print("=" * 60)
    print("Testing AnkiConnect Connection")
    print("=" * 60)

    # Test 1: Verify connection and get all deck names
    try:
        deck_names = anki_request('deckNames')
        print("\n✓ Successfully connected to Anki.")
        print(f"\nAvailable decks ({len(deck_names)}):")
        for deck in sorted(deck_names):
            print(f"  - {deck}")

        if DECK_NAME not in deck_names:
            print(f"\n⚠ Warning: Deck '{DECK_NAME}' not found.")
            return

        print(f"\n✓ Found target deck: '{DECK_NAME}'")

    except Exception as e:
        print(f"\n✗ Error: {e}")
        print("\nMake sure:")
        print("  1. Anki Desktop is running")
        print("  2. AnkiConnect add-on is installed (code: 2055492159)")
        return

    # Test 2: Get notes from the deck
    print("\n" + "=" * 60)
    print("Reading Notes from Deck")
    print("=" * 60)

    try:
        query = f'deck:"{DECK_NAME}"'
        note_ids = anki_request('findNotes', query=query)
        print(f"\n✓ Found {len(note_ids)} notes in '{DECK_NAME}'.")

        if note_ids:
            # Get info for the first 3 notes
            num_to_display = min(3, len(note_ids))
            notes_info = anki_request('notesInfo', notes=note_ids[:num_to_display])

            print(f"\nShowing first {num_to_display} notes:\n")
            for i, note in enumerate(notes_info):
                print(f"--- Note {i+1} (ID: {note['noteId']}) ---")
                print(f"Model: {note['modelName']}")
                print(f"Tags: {', '.join(note['tags']) if note['tags'] else 'None'}")
                print("Fields:")
                for field_name, field_data in note['fields'].items():
                    value = field_data['value'][:100]  # Truncate long values
                    if len(field_data['value']) > 100:
                        value += "..."
                    print(f"  {field_name}: {value}")
                print()

    except Exception as e:
        print(f"\n✗ Error reading notes: {e}")

if __name__ == "__main__":
    main()
