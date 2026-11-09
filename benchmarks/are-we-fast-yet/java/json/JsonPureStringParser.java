/* Adapted for ns-next from Are We Fast Yet
 * (https://github.com/smarr/are-we-fast-yet, commit
 * 74306fec151070fd07157cefeacf19e7e0bcdc89,
 * benchmarks/Java/src/json/JsonPureStringParser.java).
 * Changed for ns-next: names are written out; the characters are read as an
 * int by charAt, -1 at the end of the input, where upstream allocated a String
 * per character read and compared it with equals, the only port that allocated
 * a String per character.
 */
/*******************************************************************************
 * Copyright (c) 2015 Stefan Marr
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in all
 * copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
 * SOFTWARE.
 ******************************************************************************/
package json;

public final class JsonPureStringParser {

  private final String input;
  private int index;
  private int line;
  private int column;
  private int current;
  private String captureBuffer;
  private int captureStart;

  public JsonPureStringParser(final String string) {
    this.input = string;
    index = -1;
    line = 1;
    captureStart = -1;
    column = 0;
    current = -1;
    captureBuffer = "";
  }

  public JsonValue parse() {
    read();
    skipWhiteSpace();
    JsonValue result = readValue();
    skipWhiteSpace();
    if (!isEndOfText()) {
      throw error("Unexpected character");
    }
    return result;
  }

  private JsonValue readValue() {
    switch(current) {
    case 'n':
      return readNull();
    case 't':
      return readTrue();
    case 'f':
      return readFalse();
    case '"':
      return readString();
    case '[':
      return readArray();
    case '{':
      return readObject();
    case '-':
    case '0':
    case '1':
    case '2':
    case '3':
    case '4':
    case '5':
    case '6':
    case '7':
    case '8':
    case '9':
      return readNumber();
    default:
      throw expected("value");
    }
  }

  private JsonArray readArray() {
    read();
    JsonArray array = new JsonArray();
    skipWhiteSpace();
    if (readCharacter(']')) {
      return array;
    }
    do {
      skipWhiteSpace();
      array.add(readValue());
      skipWhiteSpace();
    } while (readCharacter(','));
    if (!readCharacter(']')) {
      throw expected("',' or ']'");
    }
    return array;
  }

  private JsonObject readObject() {
    read();
    JsonObject object = new JsonObject();
    skipWhiteSpace();
    if (readCharacter('}')) {
      return object;
    }
    do {
      skipWhiteSpace();
      String name = readName();
      skipWhiteSpace();
      if (!readCharacter(':')) {
        throw expected("':'");
      }
      skipWhiteSpace();
      object.add(name, readValue());
      skipWhiteSpace();
    } while (readCharacter(','));

    if (!readCharacter('}')) {
      throw expected("',' or '}'");
    }
    return object;
  }

  private String readName() {
    if (current != '"') {
      throw expected("name");
    }
    return readStringInternal();
  }

  private JsonValue readNull() {
    read();
    readRequiredCharacter('u');
    readRequiredCharacter('l');
    readRequiredCharacter('l');
    return JsonLiteral.NULL;
  }

  private JsonValue readTrue() {
    read();
    readRequiredCharacter('r');
    readRequiredCharacter('u');
    readRequiredCharacter('e');
    return JsonLiteral.TRUE;
  }

  private JsonValue readFalse() {
    read();
    readRequiredCharacter('a');
    readRequiredCharacter('l');
    readRequiredCharacter('s');
    readRequiredCharacter('e');
    return JsonLiteral.FALSE;
  }

  private void readRequiredCharacter(final char character) {
    if (!readCharacter(character)) {
      throw expected("'" + character + "'");
    }
  }

  private JsonValue readString() {
    return new JsonString(readStringInternal());
  }

  private String readStringInternal() {
    read();
    startCapture();
    while (current != '"') {
      if (current == '\\') {
        pauseCapture();
        readEscape();
        startCapture();
      } else {
        read();
      }
    }
    String string = endCapture();
    read();
    return string;
  }

  private void readEscape() {
    read();
    switch(current) {
    case '"':
    case '/':
    case '\\':
      captureBuffer += (char) current;
      break;
    case 'b':
      captureBuffer += "\b";
      break;
    case 'f':
      captureBuffer += "\f";
      break;
    case 'n':
      captureBuffer += "\n";
      break;
    case 'r':
      captureBuffer += "\r";
      break;
    case 't':
      captureBuffer += "\t";
      break;
    default:
      throw expected("valid escape sequence");
    }
    read();
  }

  private JsonValue readNumber() {
    startCapture();
    readCharacter('-');
    int firstDigit = current;
    if (!readDigit()) {
      throw expected("digit");
    }
    if (firstDigit != '0') {
      // Checkstyle: stop
      while (readDigit()) { }
     // Checkstyle: resume
    }
    readFraction();
    readExponent();
    return new JsonNumber(endCapture());
  }

  private boolean readFraction() {
    if (!readCharacter('.')) {
      return false;
    }
    if (!readDigit()) {
      throw expected("digit");
    }
    // Checkstyle: stop
    while (readDigit()) { }
    // Checkstyle: resume
    return true;
  }

  private boolean readExponent() {
    if (!readCharacter('e') && !readCharacter('E')) {
      return false;
    }
    if (!readCharacter('+')) {
      readCharacter('-');
    }
    if (!readDigit()) {
      throw expected("digit");
    }

    // Checkstyle: stop
    while (readDigit()) { }
    // Checkstyle: resume
    return true;
  }

  private boolean readCharacter(final char character) {
    if (current != character) {
      return false;
    }
    read();
    return true;
  }

  private boolean readDigit() {
    if (!isDigit()) {
      return false;
    }
    read();
    return true;
  }

  private void skipWhiteSpace() {
    while (isWhiteSpace()) {
      read();
    }
  }

  private void read() {
    if (current == '\n') {
      line++;
      column = 0;
    }
    index++;
    if (index < input.length()) {
      current = input.charAt(index);
    } else {
      current = -1;
    }
  }

  private void startCapture() {
    captureStart = index;
  }

  private void pauseCapture() {
    int end = current == -1 ? index : index - 1;
    captureBuffer += input.substring(captureStart, end + 1);
    captureStart = -1;
  }

  private String endCapture() {
    int end = current == -1 ? index : index - 1;
    String captured;
    if ("".equals(captureBuffer)) {
      captured = input.substring(captureStart, end + 1);
    } else {
      captureBuffer += input.substring(captureStart, end + 1);
      captured = captureBuffer;
      captureBuffer = "";
    }
    captureStart = -1;
    return captured;
  }

  private ParseException expected(final String expected) {
    if (isEndOfText()) {
      return error("Unexpected end of input");
    }
    return error("Expected " + expected);
  }

  private ParseException error(final String message) {
    return new ParseException(message, index, line, column - 1);
  }

  private boolean isWhiteSpace() {
    return current == ' ' || current == '\t' || current == '\n' || current == '\r';
  }

  private boolean isDigit() {
    return current == '0' ||
        current == '1' ||
        current == '2' ||
        current == '3' ||
        current == '4' ||
        current == '5' ||
        current == '6' ||
        current == '7' ||
        current == '8' ||
        current == '9';
  }

  private boolean isEndOfText() {
    return current == -1;
  }
}
