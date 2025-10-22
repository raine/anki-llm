import json
import csv
import requests

ANKI_CONNECT_URL = "http://127.0.0.1:8765"
DECK_NAME = "Glossika-ENJA [2001-3000]"
OUTPUT_FILE = "glossika_deck_export.csv"

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

def export_deck_to_csv():
    print("=" * 60)
    print(f"Exporting deck: {DECK_NAME}")
    print("=" * 60)

    try:
        # Find all notes in the deck
        query = f'deck:"{DECK_NAME}"'
        note_ids = anki_request('findNotes', query=query)
        print(f"\n✓ Found {len(note_ids)} notes in '{DECK_NAME}'.")

        if not note_ids:
            print("No notes found to export.")
            return

        # Get detailed info for all notes
        print(f"\nFetching note details...")
        notes_info = anki_request('notesInfo', notes=note_ids)
        print(f"✓ Retrieved information for {len(notes_info)} notes.")

        # Write to CSV
        print(f"\nWriting to {OUTPUT_FILE}...")
        with open(OUTPUT_FILE, 'w', newline='', encoding='utf-8') as csvfile:
            # Define the field names for CSV
            fieldnames = ['id', 'english', 'japanese', 'ka', 'ROM', 'explanation']
            writer = csv.DictWriter(csvfile, fieldnames=fieldnames, lineterminator='\n')

            writer.writeheader()

            for note in notes_info:
                fields = note['fields']
                row = {
                    'id': fields.get('Id', {}).get('value', '').replace('\r', ''),
                    'english': fields.get('English', {}).get('value', '').replace('\r', ''),
                    'japanese': fields.get('Japanese', {}).get('value', '').replace('\r', ''),
                    'ka': fields.get('か', {}).get('value', '').replace('\r', ''),
                    'ROM': fields.get('ROM', {}).get('value', '').replace('\r', ''),
                    'explanation': fields.get('Explanation', {}).get('value', '').replace('\r', '')
                }
                writer.writerow(row)

        print(f"✓ Successfully exported {len(notes_info)} notes to {OUTPUT_FILE}")

    except Exception as e:
        print(f"\n✗ Error: {e}")
        print("\nMake sure:")
        print("  1. Anki Desktop is running")
        print("  2. AnkiConnect add-on is installed (code: 2055492159)")
        print(f"  3. Deck '{DECK_NAME}' exists")

if __name__ == "__main__":
    export_deck_to_csv()
